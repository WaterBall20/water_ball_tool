//AI=== wbfp 入口解析测试 / wbfp argument-routing tests
use super::{WaterBallFilePackCommand, run};
use clap::Parser;
use pretty_assertions::assert_eq;

fn parse(args: &[&str]) -> Result<crate::command::Cli, clap::Error> {
    let mut argv = vec!["water_ball_tool"];
    argv.extend_from_slice(args);
    crate::command::Cli::try_parse_from(argv)
}

#[test]
fn bare_pack_path_parses_as_manager() -> Result<(), Box<dyn std::error::Error>> {
    let cli = parse(&["wbfp", "game.wbfp"])?;
    match cli.command {
        crate::command::Commands::Wbfp(args) => {
            assert!(args.commands.is_none());
            assert_eq!(args.pack_path.as_deref(), Some("game.wbfp"));
        }
        _ => return Err("expected the wbfp command".into()),
    }
    Ok(())
}

#[test]
fn no_subcommand_without_pack_path_parses_as_manager() -> Result<(), Box<dyn std::error::Error>> {
    let cli = parse(&["wbfp"])?;
    match cli.command {
        crate::command::Commands::Wbfp(args) => {
            assert!(args.commands.is_none());
            assert!(args.pack_path.is_none());
        }
        _ => return Err("expected the wbfp command".into()),
    }
    Ok(())
}

#[test]
fn bare_pack_path_accepts_no_tui_flag() -> Result<(), Box<dyn std::error::Error>> {
    let cli = parse(&["wbfp", "--no-tui", "game.wbfp"])?;
    match cli.command {
        crate::command::Commands::Wbfp(args) => {
            assert!(args.commands.is_none());
            assert!(args.no_tui);
        }
        _ => return Err("expected the wbfp command".into()),
    }
    Ok(())
}

#[test]
fn no_subcommand_with_no_tui_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let args = WaterBallFilePackCommand {
        commands: None,
        pack_path: Some("game.wbfp".to_string()),
        no_tui: true,
    };
    let error = run(args)
        .err()
        .ok_or("no-subcommand mode must reject --no-tui")?;
    assert_eq!(
        error.to_string(),
        "浏览/文件管理需要交互式终端 / browse and file management require an interactive terminal"
    );
    Ok(())
}

#[test]
fn subcommands_still_parse() -> Result<(), Box<dyn std::error::Error>> {
    let cases: &[&[&str]] = &[
        &["wbfp", "b", "game.wbfp"],
        &["wbfp", "pack", "src", "out.wbfp"],
        &["wbfp", "u", "game.wbfp"],
        &["wbfp", "h", "game.wbfp"],
    ];
    for argv in cases {
        parse(argv)?;
    }
    Ok(())
}
//AI_END===
