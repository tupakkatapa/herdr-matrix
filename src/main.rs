use std::process::ExitCode;

use herdr_matrix::app::{from_env, queue_prompt, serve};

const USAGE: &str = "usage: herdr-matrix
       herdr-matrix prompt <agent> <text>...";

fn main() -> anyhow::Result<ExitCode> {
    let env = |name: &str| std::env::var(name).ok();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => {}
        [command, agent, text @ ..] if command == "prompt" && !text.is_empty() => {
            queue_prompt(&env, agent, &text.join(" "))?;
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
