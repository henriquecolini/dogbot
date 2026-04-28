use clap::{FromArgMatches, Parser};
use std::any::type_name;
use teloxide::utils::command::{BotCommands, ParseError};

pub mod chmod;
pub mod chown;
pub mod connect;
pub mod disconnect;
pub mod download;
pub mod help;
pub mod hostname;
pub mod id;
pub mod ls;
pub mod mkdir;
pub mod read;
pub mod rm;
pub mod upload;
pub mod write;

use chmod::ChmodCommand;
use chown::ChownCommand;
use connect::ConnectCommand;
use download::DownloadCommand;
use hostname::HostnameCommand;
use ls::LsCommand;
use mkdir::MkdirCommand;
use read::ReadCommand;
use rm::RmCommand;
use upload::UploadCommand;
use write::WriteCommand;

#[derive(BotCommands, Clone, Debug, PartialEq)]
#[command(rename_rule = "lowercase")]
#[command(parse_with = parse_args)]
pub enum Command {
    #[command(description = "shows this message")]
    Help,
    #[command(description = "sets a unique hostname for the chat")]
    Hostname(HostnameCommand),
    #[command(description = "connects to another chat")]
    Connect(ConnectCommand),
    #[command(description = "disconnects from the current chat")]
    Disconnect,
    #[command(description = "lists files")]
    Ls(LsCommand),
    #[command(description = "creates a new directory")]
    Mkdir(MkdirCommand),
    #[command(description = "writes to a file, creating it if it doesn't exist")]
    #[command(parse_with = write::parse)]
    Write(WriteCommand),
    #[command(description = "outputs the contents of a file")]
    Read(ReadCommand),
    #[command(description = "deletes a file")]
    Rm(RmCommand),
    #[command(description = "changes the permissions of a file or directory")]
    Chmod(ChmodCommand),
    #[command(description = "changes the owner of a file")]
    Chown(ChownCommand),
    #[command(description = "shows current user and chat IDs")]
    Id,
    #[command(description = "uploads data to a file, creating it if it doesn't exist")]
    Upload(UploadCommand),
    #[command(description = "downloads a file")]
    Download(DownloadCommand),
}

fn parse_args<T: Parser>(input: String) -> Result<(T,), ParseError> {
    let Some(args) = shlex::split(&input) else {
        return Err(ParseError::IncorrectFormat(
            anyhow::anyhow!("Incorrect args format").into_boxed_dyn_error(),
        ));
    };
    let name = type_name::<T>();
    let name = name
        .split("::")
        .last()
        .map(|name| name.strip_suffix("Command").unwrap_or(name))
        .map(|name| name.to_lowercase())
        .unwrap_or_else(|| "<command>".to_string());
    let mut cmd = T::command();
    cmd.set_bin_name(format!("/{name}"));
    let mut matches = match cmd
        .no_binary_name(true)
        .try_get_matches_from(args.iter().as_ref())
    {
        Ok(m) => m,
        Err(err) => return Err(ParseError::Custom(err.into())),
    };
    match <T as FromArgMatches>::from_arg_matches_mut(&mut matches) {
        Ok(t) => Ok((t,)),
        Err(err) => Err(ParseError::Custom(err.into())),
    }
}
