use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::settings::validate_source_path;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LaunchOptions {
    pub event_logs: Vec<PathBuf>,
    pub show: bool,
    pub autostart: bool,
    pub exit: bool,
}

impl LaunchOptions {
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut args = args.into_iter();
        let _ = args.next();
        let mut options = Self::default();
        while let Some(argument) = args.next() {
            if argument == "--show" {
                options.show = true;
            } else if argument == "--autostart" {
                options.autostart = true;
            } else if argument == "--exit" {
                options.exit = true;
            } else if argument == "--event-log" {
                let value = args
                    .next()
                    .ok_or_else(|| "--event-log requires an absolute path".to_string())?;
                options.set_event_log(value)?;
            } else if let Some(value) = argument
                .to_string_lossy()
                .strip_prefix("--event-log=")
                .map(ToOwned::to_owned)
            {
                options.set_event_log(OsString::from(value))?;
            } else {
                return Err(format!(
                    "unsupported argument: {}",
                    argument.to_string_lossy()
                ));
            }
        }
        Ok(options)
    }

    pub fn show_detail(&self) -> bool {
        !self.exit && (self.show || !self.autostart)
    }

    fn set_event_log(&mut self, value: OsString) -> Result<(), String> {
        let path = validate_source_path(PathBuf::from(value))?;
        self.event_logs.push(path);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceOrigin {
    CommandLine,
    Environment,
    Saved,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitialSources {
    pub paths: Vec<PathBuf>,
    pub origin: SourceOrigin,
}

pub fn resolve_initial_sources(
    options: &LaunchOptions,
    environment: Option<OsString>,
    saved: &[String],
) -> Result<InitialSources, String> {
    if !options.event_logs.is_empty() {
        return Ok(InitialSources {
            paths: options.event_logs.clone(),
            origin: SourceOrigin::CommandLine,
        });
    }
    if let Some(path) = environment {
        return Ok(InitialSources {
            paths: vec![validate_source_path(PathBuf::from(path))?],
            origin: SourceOrigin::Environment,
        });
    }
    if !saved.is_empty() {
        let mut paths = Vec::new();
        for item in saved {
            paths.push(validate_source_path(Path::new(item))?);
        }
        return Ok(InitialSources {
            paths,
            origin: SourceOrigin::Saved,
        });
    }
    Ok(InitialSources {
        paths: Vec::new(),
        origin: SourceOrigin::None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_supported_arguments_and_launch_modes() {
        let options = LaunchOptions::parse(args(&[
            "parley-viewer.exe",
            "--event-log",
            r"C:\logs\events.jsonl",
            "--autostart",
        ]))
        .expect("arguments should parse");
        assert_eq!(
            options.event_logs,
            vec![PathBuf::from(r"C:\logs\events.jsonl")]
        );
        assert!(!options.show_detail());

        let shown = LaunchOptions::parse(args(&["parley-viewer.exe", "--autostart", "--show"]))
            .expect("arguments should parse");
        assert!(shown.show_detail());
        assert!(LaunchOptions::default().show_detail());

        let exit = LaunchOptions::parse(args(&["parley-viewer.exe", "--exit"]))
            .expect("exit should parse");
        assert!(exit.exit);
        assert!(!exit.show_detail());
    }

    #[test]
    fn rejects_invalid_or_ambiguous_arguments() {
        assert!(LaunchOptions::parse(args(&[
            "parley-viewer.exe",
            "--event-log",
            "relative.jsonl",
        ]))
        .is_err());
        assert!(LaunchOptions::parse(args(&["parley-viewer.exe", "--event-log"])).is_err());
        assert!(LaunchOptions::parse(args(&["parley-viewer.exe", "--unknown"])).is_err());
    }

    #[test]
    fn repeated_event_log_arguments_replace_saved_sources() {
        let options = LaunchOptions::parse(args(&[
            "parley-viewer.exe",
            "--event-log=C:\\one.jsonl",
            "--event-log",
            r"C:\two.jsonl",
        ]))
        .expect("repeated event logs should parse");
        assert_eq!(
            options.event_logs,
            vec![
                PathBuf::from(r"C:\one.jsonl"),
                PathBuf::from(r"C:\two.jsonl")
            ]
        );
        let source = resolve_initial_sources(
            &options,
            Some(OsString::from(r"C:\env.jsonl")),
            &[r"C:\saved.jsonl".to_string()],
        )
        .expect("cli sources should win");
        assert_eq!(source.origin, SourceOrigin::CommandLine);
        assert_eq!(source.paths, options.event_logs);
    }

    #[test]
    fn source_precedence_is_cli_environment_saved_none() {
        let options =
            LaunchOptions::parse(args(&["parley-viewer.exe", "--event-log=C:\\cli.jsonl"]))
                .expect("arguments should parse");
        let source = resolve_initial_sources(
            &options,
            Some(OsString::from(r"C:\env.jsonl")),
            &[r"C:\saved.jsonl".to_string()],
        )
        .expect("source should resolve");
        assert_eq!(source.origin, SourceOrigin::CommandLine);
        assert_eq!(source.paths, vec![PathBuf::from(r"C:\cli.jsonl")]);

        let source = resolve_initial_sources(
            &LaunchOptions::default(),
            Some(OsString::from(r"C:\env.jsonl")),
            &[r"C:\saved.jsonl".to_string()],
        )
        .expect("source should resolve");
        assert_eq!(source.origin, SourceOrigin::Environment);

        let source = resolve_initial_sources(
            &LaunchOptions::default(),
            None,
            &[r"C:\saved.jsonl".to_string()],
        )
        .expect("source should resolve");
        assert_eq!(source.origin, SourceOrigin::Saved);

        let source = resolve_initial_sources(&LaunchOptions::default(), None, &[])
            .expect("source should resolve");
        assert_eq!(source.origin, SourceOrigin::None);

        let saved = resolve_initial_sources(
            &LaunchOptions::default(),
            None,
            &[
                r"C:\saved-a.jsonl".to_string(),
                r"C:\saved-b.jsonl".to_string(),
            ],
        )
        .expect("saved list should resolve");
        assert_eq!(saved.origin, SourceOrigin::Saved);
        assert_eq!(saved.paths.len(), 2);
    }
}
