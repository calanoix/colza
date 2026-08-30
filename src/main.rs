mod color;
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

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
