# This chat's files

dogbot runs `main.dog` from here on every message sent to the chat (other than
/commands) and replies with whatever it outputs.

- `main.dog` is the script. It runs with the permissions of whoever sent the
  message, so it must be readable and executable by them: mode 750 (`rwxr-x---`)
  lets everyone in a group trigger it.
- `config/bot.json` holds optional settings: `{ "timeout": 90, "net": true }`.
- Everything else is yours. Scripts read and write files with `read` and
  `write` from `mod bot`, and load other scripts with `use f from mod lib/thing`.

Try a change without the whole chat watching:

    ssh <this chat>@dogbot            # a console that talks to main.dog
    ssh <this chat>@dogbot 'ping'     # one message, one reply

Send /mount to the bot for the exact commands, and /help for the guide and the
dog4 cheat sheet.
