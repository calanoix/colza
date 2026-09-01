mod app;
mod color;
mod magnifier;
mod portal;
mod runtime;
mod screencast;
mod token_store;

fn main() -> anyhow::Result<()> {
    app::main()
}