# dogbot

A Telegram bot where every chat gets its own filesystem and its own program.

Send a message to a chat and the bot runs that chat's `main.dog`, written in
[dog4](https://github.com/henriquecolini/dog4). The filesystem lives in
Postgres, has Unix-style ownership and permissions, and can be mounted over
SFTP so you can edit it from your own machine.

## Chat commands

| command | does |
|---|---|
| `/help` | lists the commands |
| `/hostname [name]` | sets a unique name for the chat, used for mounting and cross-chat paths |
| `/connect <chat>` · `/disconnect` | in a private chat, operate on another chat's filesystem |
| `/ls [-l] [-a] [path]` | list files |
| `/read <path>` · `/write <path> <text>` | read and write |
| `/mkdir [-p] <path>` · `/rm [-r] <paths>` · `/mv <src> <dest>` | manage files |
| `/chmod <clause> <path>` · `/chown <user> <path>` | permissions and ownership |
| `/upload <path>` · `/download <path>` | transfer files as Telegram documents |
| `/key add|list|rm` | manage SSH keys for filesystem access |
| `/mount` | print the `sshfs` command for this chat |
| `/id` | show user and chat IDs |

## Mounting a chat's filesystem

Register a public key once, then mount:

```sh
# in Telegram
/hostname myproject
/key add ssh-ed25519 AAAAC3Nz... you@laptop
/mount

# on your machine
sshfs -p 2222 \
  -o reconnect,ServerAliveInterval=15,ServerAliveCountMax=3 \
  -o uid=$(id -u),gid=$(id -g) \
  myproject@<host>:/ ~/mnt/dogbot
code ~/mnt/dogbot
```

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

If a mount does wedge — the symptom is a command on it hanging forever rather
than failing — clear it with `fusermount3 -u -z ~/mnt/dogbot` and mount again.

The SSH **username is the chat** (its `/hostname`, or `chat<id>` for a numeric
ID) and the **key is you**. A mount gives you exactly the access you already
have in that chat — the same ownership and permission bits `/ls -l` shows.
Nothing else is exposed: there is no shell and no port forwarding, only SFTP.

Any SFTP client works, so `sftp`, `scp` and `rsync` are available for bulk
edits and backups.

## Writing scripts

A chat runs `main.dog` from its own filesystem. The file must be readable *and*
executable by you (`/chmod u+x main.dog`). The script's output is the reply.

Context arrives as globals:

| global | is |
|---|---|
| `$message` | the message that triggered the run |
| `$sender` | who sent it |
| `$reply`, `$reply_from` | the message it replied to, if any |

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
| `history [count] [offset] [with_bot]` | recent messages as maps of `user`, `content`, `is_bot` |
| `version` | the running build |
| `later path` | runs the script at `path` once this message has been answered |

`later` is for upkeep nobody should wait on: summarising, reindexing, warming
a cache. The script is checked for read and execute when queued and runs in the
background after the reply is sent, with the same globals and permissions. Its
output is discarded and its errors only logged. A chat runs one deferred script
at a time, and a deferred script can't queue another.

Failures raise catchable errors rather than returning a status code:

```dog
put (try { read missing.txt } catch e { put "no file: $e.message" })
```

### Per-chat configuration

`config/bot.json`:

```json
{ "timeout": 90, "net": true }
```

`timeout` is seconds of wall clock per run; `net` controls whether `get` and
`post` exist at all. Scripts also run under dog4's sandbox limits (fuel, call
depth, output size), so a runaway loop stops on its own.

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
| `RUST_LOG` | `info` | log filter |

The SSH host key is generated on first boot and kept in the database, so
redeploying does not break clients' `known_hosts`.

## Development

```sh
docker run -d --name dogbot-test-db -e POSTGRES_PASSWORD=test -e POSTGRES_USER=test \
  -e POSTGRES_DB=dogbot_test -p 55432:5432 postgres:16

TEST_DATABASE_URL="postgresql://test:test@localhost:55432/dogbot_test" cargo test
```

Tests skip themselves when `TEST_DATABASE_URL` is unset. To point a real client
at a local server, seed a chat and key, then:

```sh
TEST_DATABASE_URL=... cargo test -- --ignored --nocapture manual_server
```
