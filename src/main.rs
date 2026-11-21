mod app;
mod color;
mod magnifier;
mod portal;
mod prefs;
mod runtime;
mod screencast;
mod text_scale;
mod token_store;

fn main() -> anyhow::Result<()> {
    app::main()
}