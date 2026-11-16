use std::process::ExitCode;

use herdr_matrix::app::{Target, from_env, queue_prompt, serve};
use herdr_matrix::{chats, gate};

const USAGE: &str = "usage: herdr-matrix
       herdr-matrix prompt <agent> <text>...
       herdr-matrix prompt --cwd <dir> <text>...
       herdr-matrix chats <rooms|read|search|send> ...
       herdr-matrix gate";

fn main() -> anyhow::Result<ExitCode> {
    let env = |name: &str| std::env::var(name).ok();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => {}
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
        [command, request @ ..] if command == "chats" => {
            return Ok(match chats::ask(&env, request) {
                Ok(answer) => {
                    println!("{}", serde_json::to_string_pretty(&answer)?);
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("{err}");
                    ExitCode::FAILURE
                }
            });
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
