mod app_minimal;
mod color;
mod magnifier;
mod portal;

use clap::{Parser, Subcommand};
use color::{Rgb, contrast_ratio, wcag_level};

#[derive(Parser)]
#[command(name = "colorpick", about = "Pick and compare colors on Wayland (GNOME/KDE) via xdg-desktop-portal")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Click a pixel on screen (via the desktop portal) and print its color.
    Pick,
    /// Compare two colors and print their WCAG contrast ratio.
    /// Colors are hex, e.g. `colorpick compare #1A2B3C #FFFFFF`.
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
    /// Minimal GUI: one color field + magnifier button, in a single eframe
    /// window (proof of concept for embedding the magnifier as a mode
    /// instead of a separate window — see app_minimal.rs).
    Gui,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // `Magnify` needs eframe/winit's event loop to run directly on the
    // real main thread (most Linux windowing backends require this and
    // may misbehave or panic otherwise), so it can't go through
    // `#[tokio::main]` like the other subcommands. We build a small,
    // short-lived tokio runtime just to make the one async portal call,
    // then drop it and run eframe synchronously on this same thread.
    if matches!(cli.command, Command::Magnify) {
        let screenshot = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(portal::capture_screen())?;

        return match magnifier::run(screenshot)? {
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

    // `Gui` also needs to own the real main thread for the same reason as
    // `Magnify` above (winit/eframe requirement on Linux).
    if matches!(cli.command, Command::Gui) {
        return app_minimal::main();
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run_async(cli))
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