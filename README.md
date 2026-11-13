# dogbot

A Telegram bot you program yourself. Every chat gets its own little Unix
filesystem and its own program.

Send a message to a chat and the bot runs that chat's `main.dog`, a script in
[dog4](https://github.com/henriquecolini/dog4), and replies with whatever it
outputs. The filesystem lives in Postgres, has Unix-style ownership and
permissions, and is reachable over SSH: mount it with sshfs and edit in your
own editor, or talk to your script from a terminal.

## Quick start

You need Telegram, an SSH key and about five minutes.

1. **Open the bot** in Telegram and press *Start*. In a private chat with the
   bot, the chat is yours to program. In a group, it belongs to the group.
2. **`/init myproject`** writes a starter `main.dog` and names the chat
   `myproject`. Send `ping` and it answers `pong`.
3. **`/key add github:yourname`** registers the SSH keys on your GitHub
   account. No GitHub? Paste a key instead: `/key add ssh-ed25519 AAAA… you@laptop`
   (the contents of `~/.ssh/id_ed25519.pub`; `ssh-keygen -t ed25519` makes one).
4. **`/mount`** prints the commands for this chat. Add the `Host dogbot`
   block it shows to `~/.ssh/config` once, then:

   ```sh
   ssh myproject@dogbot              # a console: type messages, main.dog answers
   sftp myproject@dogbot             # or browse and copy files
   sshfs myproject@dogbot:/ ~/mnt/myproject   # or mount it and use your editor
   ```

5. **Edit `main.dog`** and try it in the console before the chat sees it.

New to dog4? Read [dog4 for people who know Python or
bash](docs/dog4-cheatsheet.md) first; it's short, and it covers the four things
that trip everyone up. Then borrow from the [examples](examples/).

## How it fits together

* **A chat is a computer.** It has a filesystem rooted at `/` and one program,
  `/main.dog`, which runs once for every message that isn't a `/command`.
* **The SSH username is the chat** (its `/hostname`, or `chat<id>` for a chat
  without one) **and the key is you.** You can reach a chat only if you've sent
  a message in it, and you get exactly the access you have there.
* **Files have owners and `rwx` bits** for owner, group (everyone in the chat)
  and others (everyone else). `/ls -l` shows them and `/chmod` changes them.
* **`main.dog` runs as whoever sent the message.** It has to be readable and
  executable by them, so in a group it needs `g+rx`. `/init` sets mode 750.

## Chat commands

| command | does |
|---|---|
| `/start` | introduces the bot and the steps above |
| `/help` | lists the commands |
| `/init [name]` | writes a starter `main.dog` and `README.md`; with a name, also sets the hostname |
| `/hostname [name]` | sets a unique name for the chat: its SSH username and cross-chat prefix |
| `/connect <chat>` · `/disconnect` | in a private chat, operate on another chat's filesystem |
| `/ls [-l] [-a] [path]` | list files |
| `/read <path>` · `/write <path> <text>` | read and write |
| `/mkdir [-p] <path>` · `/rm [-r] <paths>` · `/mv <src> <dest>` | manage files |
| `/chmod <clause> <path>` · `/chown <user> <path>` | permissions and ownership |
| `/upload <path>` · `/download <path>` | transfer files as Telegram documents |
| `/key add <key \| github:user \| gitlab:user>` · `/key list` · `/key rm <n>` | manage SSH keys |
| `/mount` | how to reach this chat over SSH |
| `/id` | show user and chat IDs |

Hostnames are letters, digits, `-`, `_` and `.`, starting with a letter, since
they become SSH usernames.

## Reaching a chat over SSH

`/mount` prints all of this with the right names filled in. Once, in
`~/.ssh/config`:

```
Host dogbot
    HostName <the bot's server>
    Port 2222
```

The first connection asks you to trust the server's key; `/mount` shows its
fingerprint so you can check. If a login is refused, the server says why (an
unregistered key, a chat that doesn't exist, or one you haven't talked in yet)
instead of asking for a password, because there are none.

### Editing with your own tools

```sh
sftp myproject@dogbot        # interactive: ls, get, put, chmod
scp main.dog myproject@dogbot:/main.dog
```

`scp` works through SFTP, which is the default since OpenSSH 9; older clients
need `scp -s`. There is no shell on the server, so anything that needs one
(`rsync` to the server, `git push`, VS Code Remote-SSH) won't work. Mount the
chat and run them against the mount instead.

To mount it, on Linux (or macOS with [macFUSE](https://macfuse.github.io/)):

```sh
mkdir -p ~/mnt/myproject
sshfs myproject@dogbot:/ ~/mnt/myproject \
  -o reconnect,ServerAliveInterval=15,ServerAliveCountMax=3 \
  -o uid=$(id -u),gid=$(id -g)
code ~/mnt/myproject
```

On Windows, use `sftp` from PowerShell (OpenSSH ships with Windows), WinSCP,
or SSHFS-Win.

`uid`/`gid` map the files onto your local account. Without them a listing shows
each owner's Telegram ID, and tools that preserve ownership on save (`sed -i`,
`cp -p`) warn on every write.

`reconnect` is worth the typing: deploying the bot restarts the container and
drops every connection, and sshfs on its own leaves a mount that answers every
operation with `Input/output error` until you `fusermount3 -u` it. With
`reconnect` the mount comes back by itself once the bot is up, though files
that were open across the restart are lost. `ServerAliveInterval` is how the
client notices promptly. The server sends keepalives of its own, so a mount
that is merely idle is never dropped.

If a mount does wedge (a command on it hangs forever instead of failing),
clear it with `fusermount3 -u -z ~/mnt/myproject` and mount again.

## Testing your bot

Breaking a group's bot in front of everyone is a rite of passage you can skip.

**The console.** `ssh myproject@dogbot` opens a prompt where every line is sent
to `main.dog` as a message from you. Replies print in the terminal, and so does
anything the script `send`s, as `[#1] …`. Nothing is posted to Telegram. It is
the real runtime with the chat's real files, so a script that writes, writes.

```
$ ssh myproject@dogbot
dogbot console for myproject. Hi Ana!
myproject> ping
pong
myproject> :eval pln { upper hello }
HELLO
```

`:eval <code>` runs a line of dog4 with the chat's modules, without
`main.dog`. `:help` lists the rest. Up-arrow repeats a message.

**One message at a time**, for scripts and quick checks:

```sh
ssh myproject@dogbot 'roll 2d6'
```

The exit status is 0 when the script ran, 1 when it failed (the error goes to
stderr) and 2 when there was nothing to run. Pipe a file to run one message per
line, and the status is the worst of them:

```sh
ssh -T myproject@dogbot < test-messages.txt
```

**From Telegram.** In a private chat with the bot, `/connect myproject` makes
your messages run `myproject`'s `main.dog`, with the replies coming to you
alone. `/disconnect` goes back.

**Plain dog4.** For logic that doesn't touch the bot, the
[playground](https://henriquecolini.github.io/dog4/) runs dog4 in the browser,
and `cargo install --git https://github.com/henriquecolini/dog4 dog4` gives you
the interpreter and a REPL.

## Writing scripts

A chat runs `main.dog` from its own filesystem. The file must be readable *and*
executable by whoever sent the message: `/chmod u+x main.dog` is enough in a
private chat, and a group needs `/chmod g+rx main.dog` too. The script's output
is the reply. Messages that start with `/` are commands for the bot and never
reach the script. Every other message does, stickers, photos and polls
included; someone joining, leaving or pinning a message is recorded in the
history but runs nothing.

Context arrives as globals:

| global | is |
|---|---|
| `$message` | the message that triggered the run |
| `$sender` | who sent it |
| `$reply`, `$reply_from` | the text of the message it replied to (as it reads now, edits included), and its author |
| `$message_id`, `$reply_id` | the ids of both, for `get_message`, `download` and `image`; null when there is none |

```dog
pln "hi $sender, you said: $message"
```

### Modules

`mod` loads another `.dog` file from the filesystem. Modules need read and
execute too, because importing one runs it.

```dog
use greet from mod lib/greet;   // lib/greet.dog in this chat
use mod ./sibling;              // relative to the importing module
use truncate from mod sys/text; // the shared library, shipped with the bot
use roll from mod otherchat:lib/dice;  // another chat, by hostname
```

Cross-chat imports honour the `others` permission bits, which are off by
default — the owner has to `/chmod o+rx` a file before anyone outside the chat
can import it. Treat that as granting code execution, not just read access.

The shared library in [`syslib/`](syslib/) is compiled into the binary:
`sys/text` has string helpers, and `sys/compat` provides dog3's `j*` JSON
functions for scripts that have not been ported yet.

### The `bot` module

```dog
use read, write, ls, exists, history, version from mod bot;
```

| function | does |
|---|---|
| `read path` | file contents; raises if missing or unreadable |
| `write path content` | writes a file |
| `exists path` | whether a path exists |
| `ls path [files] [dirs]` | an **array** of names |
| `history [count] [offset] [with_bot]` | recent messages, oldest first, as maps (see below) |
| `get_message id` | one message, shaped like a `history` row, or null |
| `version` | the running build |
| `me` | the bot's account: `[id, username, name]` |
| `tag text` | labels the messages this run sends, so `history` can tell them apart |
| `download id path` | saves a message's attachment into the chat's files; returns its size |
| `image id [max_side]` | a message's picture as a `data:image/jpeg;base64,…` URL, at most `max_side` (768) pixels wide or tall |
| `later path` | runs the script at `path` once this message has been answered |
| `send text [format] [reply_to]` | sends a message to the chat right now, while the script runs; with `reply_to`, as a reply to that message |
| `send_id text [format] [reply_to]` | like `send`, but returns the message's id |
| `edit id text [format]` | rewrites a message `send_id` returned the id of |
| `delete id` | deletes a message the bot sent, by the id `send_id` returned |
| `sleep seconds` | pauses the script |

### History

The bot keeps what is said in the chat as members see it: edits replace the
text, reactions are counted, and replies point at the message they answer. A
`history` row (and `get_message`) has:

| field | is |
|---|---|
| `id` | the message's id in the chat |
| `user`, `user_id`, `is_bot` | who sent it; the bot's own messages have `user` `<bot>` |
| `content` | its text or caption, `""` if it has none |
| `date` | when it was sent, in seconds since 1970 |
| `kind` | `text`, `photo`, `sticker`, `document`, `video`, `animation`, `audio`, `voice`, `video_note`, `poll`, `location`, `venue`, `contact`, `dice`, `service` or `other` |
| `media` | details by kind, or null: a file's `file_name`, `mime_type`, `file_size`; a sticker's `emoji`; a poll's `question` and `options`; dice's `value`; a service message's `event` (`join`, `leave`, `title`, `pin`, …) |
| `reply_to` | the id of the message it replies to, or null; for a pin, the pinned message |
| `quote` | the part of that message it quoted, or the text of a message from another chat it replies to |
| `forwarded_from` | who wrote it, if it was forwarded |
| `edited` | whether it was edited |
| `reactions` | a map of emoji to how many people reacted with it |
| `tag` | what the script that sent it called it, with `tag` (bot messages only) |
| `trigger` | the message whose run sent it (bot messages only) |

A reply is stored as an id, not a copy of the text: look it up with
`get_message` to get what it says now. Telegram doesn't tell bots when someone
deletes a message, so those stay in the history; messages the bot deletes itself are
removed. Reactions only arrive while the bot is an administrator of the group.

`tag` sets a label for what the rest of the run sends, its reply included. A
script that writes its own words (a chatbot, say) can tag them, and tell them
apart from command output later:

```dog
use tag from mod bot;
tag chat;
put "hello, I'm a bot"
```

`download` and `image` refuse files over `download_limit` (below). `image`
takes photos, stickers, image files and the thumbnails of videos, animated
stickers and the like, and returns a small JPEG that a vision model can take
as an `image_url`.

### Deferred scripts

`later` is for upkeep nobody should wait on: summarising, reindexing, warming
a cache. The script is checked for read and execute when queued and runs in the
background after the reply is sent, with the same globals and permissions. Its
output is discarded and its errors only logged. A chat runs one deferred script
at a time, and a deferred script can't queue another.

### Sending messages as you go

The script's output is sent as one reply when it finishes. To say something
earlier, or in several messages, or to show progress, use `send`, `send_id`,
`edit`, `delete` and `sleep`:

```dog
use send, send_id, edit, sleep from mod bot;

send "Rolling…";
sleep 1.5;
send "<b>You rolled a 4</b>" html;
```

`format` is `plain` (the default), `markdown`, `html` or `code`. `markdown` is
Telegram's MarkdownV2, which is strict: characters like `.`, `-` and `!` outside
formatting must be escaped with `\`, or Telegram refuses the message and `send`
raises. `html` is usually easier. Neither is shortened, so mind the 4096
character limit; `plain` and `code` are cut for you.

`send` returns nothing, because in dog4 a command's value is part of the reply
and a stray message id would end up in it. Use `send_id` when you want the id, to
build a progress message:

```dog
use send_id, edit, sleep from mod bot;

id = { send_id "Working… 0%" };
for pct in [25 50 75] {
    sleep 1;
    edit $id "Working… $pct%";
}
edit $id "Done.";
```

`delete` removes a message again, for a "working…" notice that has served its
purpose or a mistake you would rather take back. It only deletes messages the
bot itself sent in that chat, never anyone else's, even where the bot is an
admin. Telegram limits when bots may delete old messages, and if it refuses,
`delete` raises with its reason. Ids don't survive the run on their own; keep
one in a file with `write` if a later message needs to delete it.

Messages sent this way appear in `history` like any other bot message, and
whatever the script prints at the end still goes out as the final reply. A run
may send, edit or delete at most 40 messages, and `sleep` counts against the chat's
`timeout`: a script that sleeps past it fails like any other that runs too long.
Deferred (`later`) scripts can send too.

Failures raise catchable errors rather than returning a status code:

```dog
put (try { read missing.txt } catch e { put "no file: $e.message" })
```

### Per-chat configuration

`config/bot.json`:

```json
{ "timeout": 90, "net": true, "history_limit": 10000, "download_limit": 5242880 }
```

`timeout` is seconds of wall clock per run; `net` controls whether `get` and
`post` exist at all. `history_limit` is how many messages the chat's history
keeps (older ones are deleted, a few at a time), and `download_limit` the
largest attachment in bytes `download` and `image` will fetch (Telegram caps it
at 20 MiB). Scripts also run under dog4's sandbox limits (fuel, call
depth, output size), so a runaway loop stops on its own.

## Examples

[`examples/`](examples/) has small bots to copy, each one runnable as a chat's
`main.dog` and each covered by the test suite:

| example | shows |
|---|---|
| [`dice.dog`](examples/dice.dog) | `roll 2d6+1`: regexes, loops, `random` |
| [`scoreboard.dog`](examples/scoreboard.dog) | state in a JSON file, maps, sorting |
| [`quotes.dog`](examples/quotes.dog) | `$reply`, saving and picking quotes |
| [`progress.dog`](examples/progress.dog) | `send_id`, `edit`, `delete` and `sleep` |
| [`github.dog`](examples/github.dog) | calling a JSON API with `fetch`, HTML replies |
| [`router/`](examples/router/) | commands split into a module, a dispatch table |

## Running it

```sh
cp .env.example .env    # DATABASE_URL, TELOXIDE_TOKEN, SFTP_*
docker compose up --build
```

| variable | default | is |
|---|---|---|
| `DATABASE_URL` | — | Postgres connection string |
| `TELOXIDE_TOKEN` | — | Telegram bot token |
| `SFTP_BIND` | `0.0.0.0:2222` | where the SFTP server listens |
| `SFTP_HOST`, `SFTP_PORT` | `localhost`, `2222` | what `/mount` tells users to connect to |
| `DOCS_URL` | this repository's README | the guide `/start` and `/help` link to |
| `RUST_LOG` | `info` | log filter |

The SSH host key is generated on first boot and kept in the database, so
redeploying does not break clients' `known_hosts`.

The bot sets its own command menu at startup. For people who have never heard
of it, give them the bot's `@username` and make sure `SFTP_HOST` is a name
they can reach; everything after that, `/start` walks them through.

## Development

```sh
docker run -d --name dogbot-test-db -e POSTGRES_PASSWORD=test -e POSTGRES_USER=test \
  -e POSTGRES_DB=dogbot_test -p 55432:5432 postgres:16

TEST_DATABASE_URL="postgresql://test:test@localhost:55432/dogbot_test" cargo test
```

Tests skip themselves when `TEST_DATABASE_URL` is unset. To point a real client
at a local server, seed a chat called `manual` with your key, then connect
with `ssh -p 52222 manual@127.0.0.1`:

```sh
MANUAL_PUBKEY="$(cat ~/.ssh/id_ed25519.pub)" TEST_DATABASE_URL=... \
  cargo test -- --ignored --nocapture manual_server
```
