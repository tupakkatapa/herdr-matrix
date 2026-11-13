use herdr_matrix::app::{from_env, serve};

fn main() -> anyhow::Result<()> {
    let mut bot = from_env(&|name| std::env::var(name).ok())?;
    serve(
        &mut || bot.tick(),
        &|line| eprintln!("{line}"),
        &mut |wait| {
            std::thread::sleep(wait);
            true
        },
    );
    Ok(())
}
