#![feature(async_closure)]
mod commands;
mod models;
mod settings;
mod utils;
use std::{
    collections::{HashMap, HashSet},
    fs::{DirEntry, File},
    io::{Error, Write},
    path::Path,
    sync::Arc,
};

use once_cell::sync::Lazy;
use rand::Rng;
use regex::Regex;
use reqwest::StatusCode;
use serenity::{
    async_trait,
    framework::standard::{
        help_commands,
        macros::{help, hook},
        Args, Command, CommandGroup, CommandResult, DispatchError, HelpOptions, OnlyIn, Reason, StandardFramework,
    },
    model::{
        channel::{Channel, Message},
        gateway::Ready,
        id::UserId,
        prelude::ReactionType,
    },
    prelude::*,
    utils::read_image,
};
use tokio::{
    task,
    time::{delay_for, Duration},
};

use crate::{
    commands::*,
    models::{
        commands::{CommandCounter, ShardManagerContainer},
        discord::{InoriChannelUtils, InoriMessageUtils, MessageCreator},
        settings::Settings,
    },
    settings::{load_settings, save_settings, setup_settings},
    utils::consts::{AUTHOR_DISC, PROG_NAME},
};

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        println!(
            "[Bot] Client started, connected as {}#{:0>4}",
            ready.user.name, ready.user.discriminator
        );

        spawn_pfp_change_thread(Arc::new(Mutex::new(ctx))).await;
    }
}

#[help]
#[individual_command_tip(
    "**Help**\nArgument keys\n`<>` - Required\n`[]` - Options\nTo get help for a specific command, subcommand or \
     group, use `help <command>`."
)]
#[suggestion_text("**Error** Unable to find command. Similar commands: `{}`")]
#[no_help_available_text("**Error** Unable to find command")]
#[command_not_found_text("**Error** Unable to find command")]
#[dm_only_text("DMs")]
#[guild_only_text("Servers")]
#[dm_and_guild_text("DMs and Servers")]
#[max_levenshtein_distance(4)]
#[indention_prefix("-")]
#[lacking_permissions("Strike")]
#[lacking_role("Strike")]
#[wrong_channel("Strike")]
#[strikethrough_commands_tip_in_dm(
    "Commands with a ~~`strikethrough`~~ require certain lacking permissions to execute."
)]
#[strikethrough_commands_tip_in_guild(
    "Commands with a ~~`strikethrough`~~ require certain lacking permissions to execute."
)]
#[embed_error_colour(MEIBE_PINK)]
#[embed_success_colour(BLURPLE)]
async fn help(
    context: &Context,
    msg: &Message,
    args: Args,
    help_options: &'static HelpOptions,
    groups: &[&'static CommandGroup],
    owners: HashSet<UserId>,
) -> CommandResult {
    // Uncomment the following line and run `help` to
    // generate a new COMMANDS.md
    // commands_to_md(groups);
    let _ = help_commands::with_embeds(context, msg, args, help_options, groups, owners).await;

    Ok(())
}

#[hook]
async fn before(ctx: &Context, msg: &Message, command_name: &str) -> bool {
    let amatch = msg
        .author
        .id
        .to_string()
        .eq(&ctx.http.get_current_user().await.unwrap().id.to_string());

    if amatch {
        let mut data = ctx.data.write().await;
        let counter = data.get_mut::<CommandCounter>().expect("Expected CommandCounter in TypeMap.");
        let entry = counter.entry(command_name.to_string()).or_insert(0);
        *entry += 1;

        if msg.attachments.is_empty() {
            msg.delete(&ctx.http).await.unwrap();
        }

        println!("[Command] Running '{}'", command_name);
    }

    amatch
}

#[hook]
async fn after(ctx: &Context, msg: &Message, command_name: &str, res: CommandResult) {
    if !msg.attachments.is_empty() {
        msg.delete(&ctx.http).await.unwrap();
    }

    match res {
        Ok(()) => println!("[Command] Finished running '{}'", command_name),
        Err(why) => println!("[Command] Finishing running '{}' with error {:?}", command_name, why),
    }
}

static NITRO_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new("(discord.com/gifts/|discordapp.com/gifts/|discord.gift/)[ ]*([a-zA-Z0-9]{16,24})").unwrap()
});
static SLOTBOT_PREFIX_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"`.*grab`").unwrap());

#[hook]
async fn normal_message(ctx: &Context, msg: &Message) {
    if let Some(code) = NITRO_REGEX.captures(&msg.content) {
        let code = code.get(2).unwrap().as_str();

        let res = reqwest::Client::new()
            .post(&format!(
                "https://discordapp.com/api/v8/entitlements/gift-codes/{}/redeem",
                code
            ))
            .header("Authorization", &ctx.http.token)
            .send()
            .await;

        if let Ok(res) = res {
            match res.status() {
                StatusCode::OK => {
                    if msg.is_private() {
                        println!(
                            "[Sniper] Successfully sniped nitro in DM's with {}#{}",
                            msg.author.name, msg.author.discriminator
                        )
                    } else {
                        let channel_name = match ctx.http.get_channel(msg.channel_id.0).await.unwrap() {
                            Channel::Guild(channel) => channel.name,
                            _ => "Unknown".to_string(),
                        };

                        let guild_name = msg
                            .guild_id
                            .unwrap()
                            .name(&ctx.cache)
                            .await
                            .unwrap_or_else(|| "Unknown".to_string());

                        println!(
                            "[Sniper] Successfully sniped nitro in [{} > {}] from {}#{}",
                            guild_name, channel_name, msg.author.name, msg.author.discriminator
                        )
                    }
                },
                StatusCode::METHOD_NOT_ALLOWED => {
                    println!("[Sniper] There was an error on Discord's side.");
                },
                StatusCode::NOT_FOUND => {
                    println!("[Sniper] Code was fake or expired.");
                },
                StatusCode::BAD_REQUEST => {
                    println!("[Sniper] Code was already redeemed.");
                },
                StatusCode::TOO_MANY_REQUESTS => {
                    println!("[Sniper] Ratelimited.");
                },
                unknown => {
                    println!("[Sniper] Received unknown response ({})", unknown.as_str());
                },
            }
        } else {
            println!("[Sniper] Erroring while POSTing nitro code");
        }
    }

    if msg.author.id.0 == 346353957029019648
        && msg
            .content
            .starts_with("Someone just dropped their wallet in this channel! Hurry and pick it up with")
    {
        let config = {
            let data = ctx.data.read().await;
            let settings = data.get::<Settings>().expect("Expected Setting in TypeMap.").lock().await;
            settings.slotbot.clone()
        };

        if msg.is_private() || !config.enabled {
            return;
        }

        // Check if it's a guild above so this will never
        // throw error
        let guild_id = msg.guild_id.unwrap();

        if (config.mode == 1 && !config.whitelisted_guilds.contains(&guild_id.0))
            || (config.mode == 2 && config.blacklisted_guilds.contains(&guild_id.0))
        {
            return;
        }

        let pfx = if config.dynamic_prefix {
            if let Some(pfx) = SLOTBOT_PREFIX_REGEX.find(&msg.content) {
                msg.content[pfx.start() + 1..pfx.end() - 5].to_string()
            } else {
                "~".to_string()
            }
        } else {
            "~".to_string()
        };

        let res = reqwest::Client::new()
            .post(&format!("https://discord.com/api/v8/channels/{}/messages", msg.channel_id.0))
            .header("Authorization", &ctx.http.token)
            .json(&serde_json::json!({ "content": format!("{}grab", pfx) }))
            .send()
            .await;

        let sniped = match res {
            Ok(res) => res.status().as_u16() == 200,
            Err(_) => false,
        };

        let channel_name = match ctx.http.get_channel(msg.channel_id.0).await.unwrap() {
            Channel::Guild(channel) => channel.name,
            _ => "Unknown".to_string(),
        };

        let guild_name = guild_id.name(&ctx.cache).await.unwrap_or_else(|| "Unknown".to_string());

        let sniped_msg = if sniped {
            format!("Sent message in [{} > {}]", guild_name, channel_name)
        } else {
            "Failed to send message".to_string()
        };
        println!("[SlotBot] {}", sniped_msg);

        return;
    }

    if msg.author.id.0 == 294882584201003009
        && msg
            .content
            .to_string()
            .eq("<:yay:585696613507399692>   **GIVEAWAY**   <:yay:585696613507399692>")
    {
        let config = {
            let data = ctx.data.read().await;
            let settings = data.get::<Settings>().expect("Expected Setting in TypeMap.").lock().await;
            settings.giveaway.clone()
        };

        if !config.enabled || msg.is_private() {
            return;
        }

        let channel_name = match ctx.http.get_channel(msg.channel_id.0).await.unwrap() {
            Channel::Guild(channel) => channel.name,
            _ => "Unknown".to_string(),
        };

        // Check if it's a guild above so unwrap() will
        // never throw error
        let guild_name = msg
            .guild_id
            .unwrap()
            .name(&ctx.cache)
            .await
            .unwrap_or_else(|| "Unknown".to_string());

        println!(
            "[Giveaway] Detected giveaway in [{} > {}] waiting {} seconds",
            guild_name, channel_name, config.delay
        );

        tokio::time::delay_for(tokio::time::Duration::from_secs(config.delay)).await;
        msg.react(&ctx.http, ReactionType::Unicode("🎉".to_string())).await.unwrap();
        println!("[Giveaway] Joined giveaway");
    }
}

#[hook]
async fn dispatch_error(ctx: &Context, msg: &Message, error: DispatchError) {
    ctx.http.delete_message(msg.channel_id.0, msg.id.0).await.unwrap();

    match error {
        DispatchError::Ratelimited(duration) => {
            let content = format!("Try this again in {} seconds.", duration.as_secs());

            println!("[Error] Ratelimit, {}", content);
            let _ = msg
                .channel_id
                .send_tmp(ctx, |m: &mut MessageCreator| m.error().title("Ratelimit").content(content))
                .await;
        },

        DispatchError::CheckFailed(_, reason) => {
            if let Reason::User(err) = reason {
                let content = match err.as_ref() {
                    "nsfw_moderate" => "This channel is not marked as NSFW and you've specified a NSFW image.\nThis \
                                        can be overriden by executing `nsfwfilter 1`"
                        .to_string(),
                    "nsfw_strict" => "This channel is not marked as NSFW and you've specified a NSFW image.\nThis can \
                                      be overriden by executing `nsfwfilter 2`"
                        .to_string(),
                    _ => {
                        let content = format!("Undocumted error, please report this to L3af#0001\nError: `{:?}``", err);
                        println!("{}", content);

                        content
                    },
                };

                let _ = msg
                    .channel_id
                    .send_tmp(ctx, |m: &mut MessageCreator| m.error().title("Error").content(content))
                    .await;
            }
        },

        DispatchError::TooManyArguments {
            max,
            given,
        } => {
            let _ = msg
                .channel_id
                .send_tmp(ctx, |m: &mut MessageCreator| {
                    m.error()
                        .title("Error")
                        .content(&format!("Too many args given!\nMaximum: {}, Given: {}", max, given))
                })
                .await;
        },

        DispatchError::NotEnoughArguments {
            min,
            given,
        } => {
            let _ = msg
                .channel_id
                .send_tmp(ctx, |m: &mut MessageCreator| {
                    m.error()
                        .title("Error")
                        .content(&format!("To few args given!\nMinimum: {}, Given: {}", min, given))
                })
                .await;
        },

        _ => {
            println!(
                "Unhandled dispatch error, please contact #L3af#0001 about this.\nError: {:?}",
                error
            );
        },
    };
}

#[hook]
async fn dynamic_prefix(ctx: &Context, _msg: &Message) -> Option<String> {
    let data = ctx.data.read().await;
    let settings = data.get::<Settings>().expect("Expected Setting in TypeMap.").lock().await;

    Some(settings.clone().command_prefix)
}

async fn spawn_pfp_change_thread(ctx: Arc<Mutex<Context>>) {
    task::spawn(async move {
        loop {
            let start_time = std::time::SystemTime::now();
            loop {
                {
                    let ctx = ctx.lock().await;
                    let data = ctx.data.read().await;
                    let settings = data.get::<Settings>().expect("Expected Setting in TypeMap.").lock().await;

                    if settings.pfp_switcher.enabled
                        && start_time.elapsed().unwrap().as_secs() >= (settings.pfp_switcher.delay * 60) as u64
                    {
                        let path = Path::new("./pfps/");
                        if path.exists() {
                            let ops = path.read_dir().unwrap().collect::<Vec<Result<DirEntry, Error>>>();
                            let new_pfp = match settings.pfp_switcher.mode {
                                0 => ops[rand::thread_rng().gen_range(0..ops.len())].as_ref(),
                                1 => {
                                    // TODO: This shit
                                    ops[rand::thread_rng().gen_range(0..ops.len())].as_ref()
                                },
                                _ => ops[rand::thread_rng().gen_range(0..ops.len())].as_ref(),
                            }
                            .unwrap();

                            let mut user = ctx.cache.current_user().await;
                            let avatar =
                                read_image(format!("./pfps/{}", new_pfp.file_name().into_string().unwrap())).unwrap();
                            user.edit(&ctx.http, |p| p.avatar(Some(&avatar))).await.unwrap();

                            println!("[PfpSwitcher] Changing pfps");
                            break;
                        }
                    }
                }

                delay_for(Duration::from_secs(60)).await;
            }
        }
    });
}

#[tokio::main]
async fn main() {
    let settings = if Path::exists(Path::new(&"config.toml")) {
        match load_settings().await {
            Ok(settings) => settings,
            Err(why) => {
                println!("[Config] Error while loading config: {}", why);

                return;
            },
        }
    } else {
        println!(
            "Welcome to {}!\n\nI was unable to find a config file\nso I'll walk you through making a new one.\n\nIf \
             you have any issues during setup or\nwhile using the bot, feel free to contact\n{} on Discord for \
             support!\n\nIf you wish to stop the bot at any time,\npress Control+C and the bot will force stop.
        \nThis will only take a minute!",
            PROG_NAME, AUTHOR_DISC
        );

        let settings = setup_settings(&toml::map::Map::new()).await;
        println!(
            "[Config] Config setup and ready to use\n[Bot] Make sure to run {}setup which will create an new server \
             and add emotes that are used throughout the bot",
            &settings.command_prefix
        );

        settings
    };

    let framework = StandardFramework::new()
        .configure(|c| {
            c.with_whitespace(true)
                .prefix("")
                .dynamic_prefix(dynamic_prefix)
                .allow_dm(true)
                .case_insensitivity(true)
                .with_whitespace(true)
                .ignore_bots(false)
                .ignore_webhooks(true)
        })
        .before(before)
        .after(after)
        .normal_message(normal_message)
        .on_dispatch_error(dispatch_error)
        .help(&HELP)
        .group(&FUN_GROUP)
        .group(&NSFW_GROUP)
        .group(&IMAGEGEN_GROUP)
        .group(&INTERACTIONS_GROUP)
        .group(&CONFIG_GROUP)
        .group(&MISCELLANEOUS_GROUP)
        .group(&UTILITY_GROUP)
        .group(&MODERATION_GROUP);

    println!("[Bot] Configured framework");

    let mut client = Client::builder(&settings.user_token)
        .event_handler(Handler)
        .framework(framework)
        .await
        .expect("Err creating client");

    {
        let mut data = client.data.write().await;
        data.insert::<CommandCounter>(HashMap::default());
        data.insert::<Settings>(Arc::new(Mutex::new(settings)));
        data.insert::<ShardManagerContainer>(Arc::clone(&client.shard_manager));
    }

    println!("[Bot] Loaded client\n[Bot] Starting client");

    if let Err(why) = client.start().await {
        println!("[Bot] Client error: {:?}", why);
    }
}

#[allow(dead_code)]
fn titlize<D: ToString>(inp: D) -> String {
    let inp = inp.to_string();

    let first = inp[..1].to_string();
    let rest = inp[1..].to_string();

    format!("{}{}", first.to_uppercase(), rest.to_lowercase())
}

#[allow(dead_code)]
fn format_command(parent_command: &str, command: &Command) -> String {
    let mut output = String::new();

    let names = command.options.names;
    let mut cmd_name = format!("{} {}", parent_command, titlize(names.get(0).unwrap_or(&"Unknown")));
    cmd_name = cmd_name.trim().to_string();

    output = format!("{}\n\n### {}", output, cmd_name);
    if let Some(desc) = command.options.desc {
        output = format!("{}\n\n{}", output, desc);
    }

    let mut ending = String::new();
    if names.len() >= 2 {
        let mut names = names.to_vec();
        names.remove(0);
        ending = format!("- Aliases: `{}`", names.join("`, `"));
    }

    if let Some(usage) = command.options.usage {
        ending = format!("{}\n- Usage: `{} {}`", ending, cmd_name.to_lowercase(), usage);
    }

    if !command.options.examples.is_empty() {
        let examples = command.options.examples.to_vec();
        ending = format!(
            "{}\n- Examples:\n  - `{} {}`",
            ending,
            cmd_name.to_lowercase(),
            examples.join(&format!("`\n  - `{} ", cmd_name.to_lowercase()))
        );
    }

    if !command.options.sub_commands.is_empty() {
        let subcmds = command.options.sub_commands.to_vec();
        let mapped = subcmds
            .iter()
            .map(|cmd| {
                let name = cmd.options.names.get(0).unwrap_or(&"Unknown");
                let ending = format!("#{}{}", parent_command.replace(' ', " ").to_lowercase(), name.to_lowercase());

                format!("[{}]({})", name, ending)
            })
            .collect::<Vec<String>>();

        ending = format!("{}\n- Subcommands: {}", ending, mapped.join(", "));
    }

    let only_in = match command.options.only_in {
        OnlyIn::Dm => "DMs",
        OnlyIn::Guild => "Guilds",
        OnlyIn::None => "DMs and Guilds",
        _ => "Unknown",
    };

    ending = format!("{}\n- In: {}", ending, only_in);
    output = format!("{}\n\n{}", output, ending.trim());

    for sub_command in command.options.sub_commands {
        output = format!("{}{}", output, format_command(&cmd_name, sub_command));
    }

    output
}

#[allow(dead_code)]
fn commands_to_md(groups: &[&'static CommandGroup]) {
    let groups = groups.to_vec();
    let mut output = String::new();

    for group in groups {
        output = format!("{}\n\n\n## {}", output, titlize(group.name));

        for command in group.options.commands {
            output = format!("{}\n\n{}", output, format_command("", command).trim());
        }
    }

    output = format!(
        "{}\n\nThis file was autogenerated using commands_to_md in [main.rs](src/main.rs) using the commands help \
         menus\n",
        output.trim()
    );

    let mut file = File::create("COMMANDS.md").expect("Unable to create COMMANDS.md");
    file.write_all(output.as_bytes()).expect("Unable to write to COMMANDS.md");

    println!("Output saved to COMMANDS.md")
}
