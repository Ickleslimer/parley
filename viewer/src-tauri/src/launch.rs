use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::settings::validate_source_path;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LaunchOptions {
    pub event_log: Option<PathBuf>,
    pub show: bool,
    pub autostart: bool,
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
        self.show || !self.autostart
    }

    fn set_event_log(&mut self, value: OsString) -> Result<(), String> {
        if self.event_log.is_some() {
            return Err("--event-log may be provided only once".to_string());
        }
        self.event_log = Some(validate_source_path(PathBuf::from(value))?);
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
pub struct InitialSource {
    pub path: Option<PathBuf>,
    pub origin: SourceOrigin,
}

pub fn resolve_initial_source(
    options: &LaunchOptions,
    environment: Option<OsString>,
    saved: Option<&str>,
) -> Result<InitialSource, String> {
    if let Some(path) = options.event_log.as_ref() {
        return Ok(InitialSource {
            path: Some(path.clone()),
            origin: SourceOrigin::CommandLine,
        });
    }
    if let Some(path) = environment {
        return Ok(InitialSource {
            path: Some(validate_source_path(PathBuf::from(path))?),
            origin: SourceOrigin::Environment,
        });
    }
    if let Some(path) = saved {
        return Ok(InitialSource {
            path: Some(validate_source_path(Path::new(path))?),
            origin: SourceOrigin::Saved,
        });
    }
    Ok(InitialSource {
        path: None,
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
            options.event_log,
            Some(PathBuf::from(r"C:\logs\events.jsonl"))
        );
        assert!(!options.show_detail());

        let shown = LaunchOptions::parse(args(&["parley-viewer.exe", "--autostart", "--show"]))
            .expect("arguments should parse");
        assert!(shown.show_detail());
        assert!(LaunchOptions::default().show_detail());
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
        assert!(LaunchOptions::parse(args(&[
            "parley-viewer.exe",
            "--event-log=C:\\one.jsonl",
            "--event-log=C:\\two.jsonl",
        ]))
        .is_err());
        assert!(LaunchOptions::parse(args(&["parley-viewer.exe", "--unknown"])).is_err());
    }

    #[test]
    fn source_precedence_is_cli_environment_saved_none() {
        let options =
            LaunchOptions::parse(args(&["parley-viewer.exe", "--event-log=C:\\cli.jsonl"]))
                .expect("arguments should parse");
        let source = resolve_initial_source(
            &options,
            Some(OsString::from(r"C:\env.jsonl")),
            Some(r"C:\saved.jsonl"),
        )
        .expect("source should resolve");
        assert_eq!(source.origin, SourceOrigin::CommandLine);
        assert_eq!(source.path, Some(PathBuf::from(r"C:\cli.jsonl")));

        let source = resolve_initial_source(
            &LaunchOptions::default(),
            Some(OsString::from(r"C:\env.jsonl")),
            Some(r"C:\saved.jsonl"),
        )
        .expect("source should resolve");
        assert_eq!(source.origin, SourceOrigin::Environment);

        let source =
            resolve_initial_source(&LaunchOptions::default(), None, Some(r"C:\saved.jsonl"))
                .expect("source should resolve");
        assert_eq!(source.origin, SourceOrigin::Saved);

        let source = resolve_initial_source(&LaunchOptions::default(), None, None)
            .expect("source should resolve");
        assert_eq!(source.origin, SourceOrigin::None);
    }
}
