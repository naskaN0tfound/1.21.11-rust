//! Minimal launcher: finds the game executable, prepares a game folder and runs it.
//!
//! Usage: launcher [--game-bin PATH] [--game-dir DIR] [--check] [-- GAME_ARGS...]
//! The game binary can also come from the RUST121_BIN environment variable.

use std::env;
use std::path::PathBuf;
use std::process::{exit, Command};

const DEFAULT_BIN: &str = "target/release/client";

fn main() {
    let mut bin = env::var_os("RUST121_BIN").map(PathBuf::from);
    let mut dir = PathBuf::from("game");
    let mut check = false;
    let mut game_args: Vec<String> = Vec::new();

    let mut args = env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--game-bin" => bin = args.next().map(PathBuf::from),
            "--game-dir" => dir = args.next().map(PathBuf::from).unwrap_or(dir),
            "--check" => check = true,
            "--" => {
                game_args.extend(args.by_ref());
                break;
            }
            "-h" | "--help" => {
                println!("launcher [--game-bin PATH] [--game-dir DIR] [--check] [-- GAME_ARGS...]");
                return;
            }
            other => {
                eprintln!("unknown argument: {other}");
                exit(2);
            }
        }
    }

    let bin = bin.unwrap_or_else(|| PathBuf::from(DEFAULT_BIN));
    if !bin.is_file() {
        eprintln!(
            "game executable not found at {}.\nThe 1.21.11 client is not built yet; \
             pass --game-bin or set RUST121_BIN once it exists.",
            bin.display()
        );
        exit(1);
    }
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("cannot create game folder {}: {e}", dir.display());
        exit(1);
    }
    println!("game: {}\nfolder: {}", bin.display(), dir.display());
    if check {
        return;
    }

    match Command::new(&bin).current_dir(&dir).args(&game_args).status() {
        Ok(s) => exit(s.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("failed to start {}: {e}", bin.display());
            exit(1);
        }
    }
}
