mod app;
mod color;
mod magnifier;
mod portal;
mod runtime;
mod screencast;
mod token_store;

use clap::{Parser, Subcommand};
use color::{Rgb, contrast_ratio, wcag_level};

#[derive(Parser)]
#[command(name = "colza", about = "Pick and compare colors on Wayland (GNOME/KDE) via xdg-desktop-portal")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Click a pixel on screen (via the desktop portal) and print its color.
    Pick,
    /// Compare two colors and print their WCAG contrast ratio.
    /// Colors are hex, e.g. `colza compare #1A2B3C #FFFFFF`.
    Compare {
        color_a: String,
        color_b: String,
        /// Evaluate against the WCAG large-text thresholds instead of normal text.
        #[arg(long)]
        large_text: bool,
    },
    /// Click two pixels on screen (via the desktop portal) and compare them.
    PickAndCompare {
        #[arg(long)]
        large_text: bool,
    },
    /// Open a fullscreen pixel-zoom magnifier to pick a color precisely.
    /// Move the mouse to preview, left-click (or Enter) to pick, Esc to cancel.
    Magnify,
    /// Full GUI: color fields, swap, magnifier picking, and WCAG badges.
    Gui,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // `Magnify` and `Gui` need eframe/winit's event loop to run directly on
    // the real OS main thread, so they can't go through `#[tokio::main]`
    // like the other subcommands. Instead we `block_on` the async portal
    // call on the shared runtime, then run eframe synchronously here.
    //
    // The runtime is `runtime::shared()` rather than one built and dropped
    // locally: ashpd caches its D-Bus connection process-wide, bound to the
    // reactor it first saw. See runtime.rs.
    if matches!(cli.command, Command::Magnify) {
        let (capture, first_frame) = runtime::shared()?.block_on(screencast::open_best())?;

        return match magnifier::run(first_frame, capture)? {
            Some(color) => {
                println!("{color}");
                Ok(())
            }
            None => {
                println!("Cancelled.");
                Ok(())
            }
        };
    }

    if matches!(cli.command, Command::Gui) {
        return app::main();
    }

    runtime::shared()?.block_on(run_async(cli))
}

async fn run_async(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Pick => {
            let color = portal::pick_color().await?;
            println!("{color}");
        }
        Command::Compare {
            color_a,
            color_b,
            large_text,
        } => {
            let a = Rgb::from_hex(&color_a)?;
            let b = Rgb::from_hex(&color_b)?;
            print_comparison(a, b, large_text);
        }
        Command::PickAndCompare { large_text } => {
            println!("Click the first color...");
            let a = portal::pick_color().await?;
            println!("First color:  {a}");

            println!("Click the second color...");
            let b = portal::pick_color().await?;
            println!("Second color: {b}");

            print_comparison(a, b, large_text);
        }
        Command::Magnify => unreachable!("handled in main() before the async runtime starts"),
        Command::Gui => unreachable!("handled in main() before the async runtime starts"),
    }

    Ok(())
}

fn print_comparison(a: Rgb, b: Rgb, large_text: bool) {
    let ratio = contrast_ratio(a, b);
    let level = wcag_level(ratio, large_text);
    let mode = if large_text { "large text" } else { "normal text" };

    println!("A: {a}");
    println!("B: {b}");
    println!("Contrast ratio: {ratio:.2}:1");
    println!("WCAG ({mode}): {level}");
}