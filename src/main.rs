use std::path::Path;
use std::process::ExitCode;

use herdr_matrix::app::{Target, from_env, queue_file, queue_prompt, serve};
use herdr_matrix::{chats, gate};

const USAGE: &str = "usage: herdr-matrix                                run the bot, as its service does
       herdr-matrix prompt <agent> <text>...       queue a prompt for an agent; dropped when the agent is working or waits on a choice, when the name matches no agent or several, or after 10 minutes
       herdr-matrix prompt --cwd <dir> <text>...   the same, for the one agent working in <dir>
       herdr-matrix send <file> [caption]...       post a file to the owner's chat, from a herdr pane
       herdr-matrix chats rooms|read|search|send   the owner's bridged chats, through the gate
       herdr-matrix gate                           answer one request of the gate, on stdin and stdout
       herdr-matrix help                           this text";

/// True for an argument after a command that asks for the usage; alone,
/// `help` asks too.
fn help(arg: &str) -> bool {
    matches!(arg, "-h" | "--help")
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("herdr-matrix: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<ExitCode> {
    let env = |name: &str| std::env::var(name).ok();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] if env("HERDR_MATRIX_HOMESERVER").is_none() && env("HERDR_MATRIX_TOKEN").is_none() => {
            eprintln!("{USAGE}");
            return Ok(ExitCode::from(2));
        }
        [] => {}
        [arg] if help(arg) || arg == "help" => {
            println!("{USAGE}");
            return Ok(ExitCode::SUCCESS);
        }
        [command, arg] if help(arg) && matches!(command.as_str(), "send" | "prompt") => {
            println!("{USAGE}");
            return Ok(ExitCode::SUCCESS);
        }
        [command, arg] if (help(arg) || arg == "help") && command == "chats" => {
            println!("{}", gate::USAGE);
            return Ok(ExitCode::SUCCESS);
        }
        [command, flag, dir, text @ ..]
            if command == "prompt" && flag == "--cwd" && !text.is_empty() =>
        {
            queue_prompt(&env, Target::Cwd(dir), &text.join(" "))?;
            return Ok(ExitCode::SUCCESS);
        }
        [command, agent, text @ ..]
            if command == "prompt" && agent != "--cwd" && !text.is_empty() =>
        {
            queue_prompt(&env, Target::Agent(agent), &text.join(" "))?;
            return Ok(ExitCode::SUCCESS);
        }
        [command, file, caption @ ..] if command == "send" => {
            queue_file(&env, Path::new(file), &caption.join(" "))?;
            return Ok(ExitCode::SUCCESS);
        }
        [command, request @ ..] if command == "chats" => {
            let answer = chats::ask(&env, request)?;
            println!("{}", serde_json::to_string_pretty(&answer)?);
            return Ok(ExitCode::SUCCESS);
        }
        [command] if command == "gate" => {
            gate::serve(
                &env,
                &mut std::io::stdin().lock(),
                &mut std::io::stdout().lock(),
                &|line| eprintln!("{line}"),
            )?;
            return Ok(ExitCode::SUCCESS);
        }
        _ => {
            eprintln!("{USAGE}");
            return Ok(ExitCode::from(2));
        }
    }
    let mut bot = from_env(&env)?;
    bot.log_start();
    serve(
        &mut || bot.tick(),
        &|line| eprintln!("{line}"),
        &mut |wait| {
            std::thread::sleep(wait);
            true
        },
    );
    Ok(ExitCode::SUCCESS)
}
