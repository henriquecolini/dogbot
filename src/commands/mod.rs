use crate::chat_address::ChatAddress;
use crate::result::BotResult;
use crate::{PgPool, model::*};
use diesel::{PgConnection, QueryResult};
use log::*;
use teloxide::prelude::*;
use teloxide::types::ChatKind;
use teloxide::utils::command::BotCommands;

#[derive(BotCommands, Clone, Debug, PartialEq)]
#[command(rename_rule = "lowercase")]
pub enum Command {
    #[command(description = "Mostra esta mensagem")]
    Help,
    #[command(description = "Inicia um chat com o bot")]
    Start(String),
    #[command(description = "Acessa o bot através de outro chat")]
    Connect(String),
    #[command(description = "Para de acessar o bot de outro chat")]
    Disconnect,
    #[command(description = "Lista os arquivos")]
    Ls(String),
    #[command(description = "Cria pastas")]
    Mkdir(String),
    #[command(description = "Escreve um arquivo")]
    Write(String),
    #[command(description = "Lê um arquivo")]
    Read(String),
    #[command(description = "Deleta um arquivo")]
    Rm(String),
    #[command(description = "Deleta um arquivo ou pasta recursivamente (com -r)")]
    Rmr(String),
    #[command(description = "Altera as permissões de um arquivo")]
    Chmod(String),
    #[command(description = "Altera o dono de um arquivo")]
    Chown(String),
    #[command(description = "Mostra o ID da pessoa que enviou a mensagem e do chat")]
    Id,
}

pub async fn handle_command(
    bot: Bot,
    pool: PgPool,
    msg: Message,
    command: Command,
) -> BotResult<()> {
    info!("Received command: {:?}", command);
    match command {
        Command::Help => {
            help_command(bot, msg).await?;
        }
        Command::Start(alias) => {
            start_command(bot, pool, msg, alias).await?;
        }
        Command::Connect(address) => {
            connect_command(bot, pool, msg, address).await?;
        }
        Command::Disconnect => {
            disconnect_command(bot, pool, msg).await?;
        }
        Command::Ls(path) => {
            ls_command(bot, pool, msg, path).await?;
        }
        Command::Mkdir(path) => {
            mkdir_command(bot, pool, msg, path).await?;
        }
        Command::Write(path) => {
            write_command(bot, pool, msg, path).await?;
        }
        Command::Read(path) => {
            read_command(bot, pool, msg, path).await?;
        }
        Command::Rm(path) => {
            rm_command(bot, pool, msg, path, false).await?;
        }
        Command::Rmr(path) => {
            rm_command(bot, pool, msg, path, true).await?;
        }
        Command::Chmod(_) => {}
        Command::Chown(_) => {}
        Command::Id => {
            id_command(bot, msg).await?;
        }
    }
    Ok(())
}

async fn help_command(bot: Bot, msg: Message) -> BotResult<()> {
    bot.send_message(msg.chat.id, Command::descriptions().to_string())
        .await?;
    Ok(())
}

async fn start_command(bot: Bot, pool: PgPool, msg: Message, alias: String) -> BotResult<()> {
    let mut cn = pool.get()?;
    let (chat_name, is_group) = match msg.chat.kind {
        ChatKind::Public(chat) => (chat.title, true),
        ChatKind::Private(chat) => (chat.first_name, false),
    };
    let existed = chat::Chat::check_if_exists(&mut cn, msg.chat.id.0)?;
    let alias = alias.trim();
    let alias = if alias.is_empty() { None } else { Some(alias) };
    if alias.is_some() && !is_group {
        bot.send_message(
            msg.chat.id,
            "Você só pode iniciar um chat com um alias se este for um grupo",
        )
        .await?;
        return Ok(());
    }
    chat::Chat::upsert(&mut cn, msg.chat.id.0, chat_name.as_deref(), is_group)?;
    if alias.is_some() {
        chat::Chat::set_alias(&mut cn, msg.chat.id.0, alias)?;
    }
    if !existed {
        if let Some(alias) = alias {
            bot.send_message(msg.chat.id, format!("Chat criado com sucesso ({alias})"))
                .await?;
        } else {
            bot.send_message(msg.chat.id, "Chat criado com sucesso")
                .await?;
        }
    } else {
        if let Some(alias) = alias {
            bot.send_message(
                msg.chat.id,
                format!("Chat atualizado com sucesso ({alias})"),
            )
            .await?;
        } else {
            bot.send_message(msg.chat.id, "Chat atualizado com sucesso")
                .await?;
        }
    }
    Ok(())
}

async fn connect_command(bot: Bot, pool: PgPool, msg: Message, address: String) -> BotResult<()> {
    let mut cn = pool.get()?;
    let Some(user) = msg.from else {
        bot.send_message(
            msg.chat.id,
            "Apenas usuários podem se conectar a outros chats",
        )
        .await?;
        return Ok(());
    };
    if !msg.chat.is_private() {
        bot.send_message(
            msg.chat.id,
            "Você só pode se conectar a outro chat através de seu chat privado",
        )
        .await?;
        return Ok(());
    }
    let address = ChatAddress::parse(&address);
    let chat = chat::Chat::find(&mut cn, address)?;
    if let Some(chat) = chat {
        if user_in_chat::UserInChat::exists(&mut cn, user.id.0 as i64, chat.id)? {
            user::User::set_connection(&mut cn, user.id.0 as i64, Some(chat.id))?;
            bot.send_message(msg.chat.id, format!("Conectado a {chat} ({address})"))
                .await?;
        } else {
            bot.send_message(msg.chat.id, "Você não possui acesso à este chat")
                .await?;
        }
    } else {
        bot.send_message(msg.chat.id, "Chat não encontrado").await?;
    }
    Ok(())
}

async fn disconnect_command(bot: Bot, pool: PgPool, msg: Message) -> BotResult<()> {
    let mut cn = pool.get()?;
    let Some(user) = msg.from else {
        bot.send_message(msg.chat.id, "Apenas usuários podem se desconectar de chats")
            .await?;
        return Ok(());
    };
    if !msg.chat.is_private() {
        bot.send_message(
            msg.chat.id,
            "Você só pode se desconectar de um chat através de seu chat privado",
        )
        .await?;
        return Ok(());
    }
    user::User::set_connection(&mut cn, user.id.0 as i64, None)?;
    bot.send_message(msg.chat.id, "Desconectado com sucesso")
        .await?;
    Ok(())
}

fn get_connected_chat(cn: &mut PgConnection, user_id: i64, chat_id: i64) -> QueryResult<i64> {
    let user = user::User::get(cn, user_id)?;
    Ok(user.current_connection.unwrap_or(chat_id))
}

async fn ls_command(bot: Bot, pool: PgPool, msg: Message, path: String) -> BotResult<()> {
    let mut cn = pool.get()?;
    let Some(user_id) = msg.from.map(|u| u.id.0 as i64) else {
        return Ok(());
    };
    let chat_id = get_connected_chat(&mut cn, user_id, msg.chat.id.0)?;
    match crate::files::list(&mut cn, chat_id, user_id, &path) {
        Ok(mut files) => {
            if files.is_empty() {
                bot.send_message(msg.chat.id, "A pasta está vazia").await?;
                return Ok(());
            }
            files.sort_by(|a, b| b.is_dir().cmp(&a.is_dir()).then(a.name.cmp(&b.name)));
            bot.send_message(
                msg.chat.id,
                files
                    .iter()
                    .map(|f| {
                        if f.is_dir() {
                            format!("📂 {}/", f.name)
                        } else {
                            format!("📄 {}", f.name)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
            .await?;
        }
        Err(err) => {
            bot.send_message(msg.chat.id, format!("Erro ao listar arquivos: {}", err))
                .await?;
        }
    }
    Ok(())
}

async fn read_command(bot: Bot, pool: PgPool, msg: Message, path: String) -> BotResult<()> {
    let mut cn = pool.get()?;
    let Some(user_id) = msg.from.map(|u| u.id.0 as i64) else {
        return Ok(());
    };
    let chat_id = get_connected_chat(&mut cn, user_id, msg.chat.id.0)?;
    match crate::files::read(&mut cn, chat_id, user_id, &path) {
        Ok(content) => {
            bot.send_message(msg.chat.id, String::from_utf8_lossy(&content))
                .await?;
        }
        Err(err) => {
            bot.send_message(msg.chat.id, format!("Erro ao ler arquivo: {}", err))
                .await?;
        }
    }
    Ok(())
}

async fn write_command(
    bot: Bot,
    pool: PgPool,
    msg: Message,
    path_and_content: String,
) -> BotResult<()> {
    let mut cn = pool.get()?;
    let Some(user_id) = msg.from.map(|u| u.id.0 as i64) else {
        return Ok(());
    };
    let chat_id = get_connected_chat(&mut cn, user_id, msg.chat.id.0)?;
    let (path, content) = path_and_content.split_at(
        path_and_content
            .find(|c: char| c.is_whitespace())
            .unwrap_or(path_and_content.len()),
    );
    match crate::files::write(&mut cn, chat_id, user_id, path.trim(), content.as_bytes()) {
        Ok(file) => {
            bot.send_message(msg.chat.id, format!("Arquivo criado: {}", file.name))
                .await?;
        }
        Err(err) => {
            bot.send_message(msg.chat.id, format!("Erro ao criar arquivo: {}", err))
                .await?;
        }
    }
    Ok(())
}

async fn rm_command(bot: Bot, pool: PgPool, msg: Message, path: String, recursive: bool) -> BotResult<()> {
    let mut cn = pool.get()?;
    let Some(user_id) = msg.from.map(|u| u.id.0 as i64) else {
        return Ok(());
    };
    let chat_id = get_connected_chat(&mut cn, user_id, msg.chat.id.0)?;
    match crate::files::remove(&mut cn, chat_id, user_id, &path, recursive) {
        Ok(_) => {
            bot.send_message(msg.chat.id, "Arquivo deletado com sucesso")
                .await?;
        }
        Err(err) => {
            bot.send_message(msg.chat.id, format!("Erro ao deletar arquivo: {}", err))
                .await?;
        }
    }
    Ok(())
}

async fn mkdir_command(bot: Bot, pool: PgPool, msg: Message, path: String) -> BotResult<()> {
    let mut cn = pool.get()?;
    let Some(user_id) = msg.from.map(|u| u.id.0 as i64) else {
        return Ok(());
    };
    let chat_id = get_connected_chat(&mut cn, user_id, msg.chat.id.0)?;
    match crate::files::mkdir(&mut cn, chat_id, user_id, &path) {
        Ok(Some(dir)) => {
            bot.send_message(msg.chat.id, format!("Pastas criadas: {}", dir.name))
                .await?;
        }
        Ok(None) => {
            bot.send_message(msg.chat.id, "Pastas criadas").await?;
        }
        Err(err) => {
            bot.send_message(msg.chat.id, format!("Erro ao criar pastas: {}", err))
                .await?;
        }
    }
    Ok(())
}

async fn id_command(bot: Bot, msg: Message) -> BotResult<()> {
    bot.send_message(
        msg.chat.id,
        format!(
            "Seu ID: `{}`, ID do Chat: `{}`",
            msg.from.map(|u| u.id.0).unwrap_or_default(),
            msg.chat.id
        ),
    )
    .await?;
    Ok(())
}
