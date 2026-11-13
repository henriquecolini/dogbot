# dog4 for people who know Python or bash

dogbot scripts are written in [dog4](https://github.com/henriquecolini/dog4).
It looks like a shell and behaves like neither a shell nor Python, so this page
is the short list of what to unlearn. Everything here was run through dog4; you
can paste any snippet into the
[playground](https://henriquecolini.github.io/dog4/) to poke at it.

The precise rules are in the
[language reference](https://github.com/henriquecolini/dog4/blob/main/docs/language.md).

## The one idea: output *is* the reply

A dog4 program doesn't `return` a reply or `print` one. Every statement that
runs a command **outputs** that command's value, and the script's reply is all
of those outputs combined. A script that outputs nothing sends no reply.

```dog
put "hello";        // outputs "hello"
pln "and bye"       // outputs "and bye\n"
```

Two consequences catch everyone:

**Outputs are combined with `+`**, which for numbers means adding:

```dog
put 1; put 2        // replies "3", not "12"
put "1"; put "2"    // replies "12"
pln 1; pln 2        // replies "1\n2\n": pln makes text
```

**Any command you call is part of the reply**, even one you only wanted for
its effect. Assign its value, or wrap it in a block, to keep it out:

```dog
xs = [b c];
insert $xs 0 a;            // oops: [a b c] is now in the reply
xs = { insert $xs 0 a };   // fine: an assignment outputs nothing
```

## `if`, `for` and friends throw their value away

As statements, `if`, `for`, `while`, `try` and bare `{ … }` blocks run, but
their output is discarded. This is the most common "why doesn't my bot
answer?":

```dog
if true { put yes }          // replies nothing
put (if true { put yes })    // replies "yes"
if true { println yes }      // replies "yes": println writes immediately
```

Two styles work, pick one:

* **Expression style**: build the reply as a value and `put` it once, as in
  the `/init` starter: `put (if … { put a } else if … { put b })`. When no
  branch matches, the `if` is `null` and nothing is sent. Don't write
  `else { null }`: `null` on its own is read as a command and fails.
* **Imperative style**: use `println` / `print` anywhere, as you would
  `print()` in Python.

`return` works at the top level too, and ends the script with that reply:

```dog
if (!{ starts_with $message "!" }) { return };   // no reply
```

## Statements end with `;`, not with newlines

```dog
pln a
pln b               // replies "a pln b": one command with three arguments
```

Put `;` after every statement. It's optional only after a block-ish statement
(`if … { }`, `for`, `fn`) and after the last statement of a block.

## Two modes: commands and expressions

Code is read in **command mode** by default, like a shell: a name followed by
space-separated arguments, `len $xs`, `replace $text a b`. Bare words are text:
`hello`, `data/file.json`, `ping`.

Inside `( … )`, after `=`, and after `return`, dog4 reads **expressions**,
like Python: operators, `$x + 1`, `$a == $b`, calls written `f(x, y)`.

| you want | write | not |
|---|---|---|
| a command's value inside an expression | `{ len $xs }` or `len($xs)` | `(len $xs)` (an error that says so) |
| arithmetic or comparison as an argument | `pln ($n + 1)` | `pln $n + 1` (prints `5 + 1`) |
| a condition | `if ($n > 3) { … }` | `if $n > 3 { … }` |
| a path or file name after `=` | `file = "data/scores.json"` | `file = data/scores.json` (that's *division*) |

A plain value needs no parentheses: `if $flag { … }`, `pln $name`.

## Strings

```dog
name = Ana;
pln "hi $name"                   // hi Ana
pln "${name}.txt"                // Ana.txt — "$name.txt" would read a field
pln "n = $({ len [a b] })"       // n = 2: $( … ) for anything computed
pln "n = { len [a b] }"          // braces in a string are just text
pln """\d+ stays \d+"""          // raw string: no escapes, handy for regexes
```

Message text is always a string, and `==` never converts types:

```dog
pln ("1" == 1)           // false
pln (num("1") == 1)      // true; num("abc") raises an error
```

## Variables, arrays, maps

```dog
x = 1;                   // assign: no $
pln $x;                  // read: $
xs = [a b c];            // array: space or comma separated
pln $xs[0] $xs[-1..];    // a [c]  (slices clamp, never fail)
xs += [d];               // append
d = [name = Ana, age = 30];
pln $d.name ($d.missing ?? "default") $d["age"];
d.age = 31;
```

Builtins never change their arguments; they return a new value. Write
`xs = { sort $xs }`, not `sort $xs`.

`for` over a string goes word by word, which suits chat messages:

```dog
for w in $message { println "<$w>" };
```

## Translation table

| Python | bash | dog4 |
|---|---|---|
| `print(x)` | `echo $x` | `pln $x` (part of the reply) |
| `f"hi {name}"` | `"hi $name"` | `"hi $name"` |
| `len(xs)` | `${#xs[@]}` | `{ len $xs }` or `len($xs)` |
| `xs.append(y)` | `xs+=("$y")` | `xs += [$y]` |
| `d.get("k", 0)` | | `$d.k ?? 0` |
| `s.split()` | `read -a` | `{ split $s }` |
| `" ".join(xs)` | | `{ join $xs " " }` |
| `int(s)` | `$((s))` | `num($s)` |
| `if a > b:` | `if [ $a -gt $b ]` | `if ($a > $b) { … }` |
| `elif` | `elif` | `else if` |
| `for x in xs:` | `for x in …` | `for x in $xs { … }` |
| `for i in range(3):` | `for i in {0..2}` | `for i in (0..3) { … }` |
| `def add(a, b): return a + b` | | `fn add (a, b) ($a + $b)` |
| `try: … except E as e:` | `cmd \|\| …` | `try { … } catch e { … }` (`$e.message`) |
| `raise ValueError("x")` | `exit 1` | `fail x` |
| `json.loads(s)` | `jq` | `json.parse($s)` |
| `re.search(r"(\d+)", s)` | `grep -o` | `{ captures $s """(\d+)""" }` |
| `random.randint(1, 6)` | `$((RANDOM % 6 + 1))` | `{ random 1 7 }` (upper bound excluded) |
| `requests.get(url).text` | `curl $url` | `{ get $url }` |
| `import mylib` | `source mylib.sh` | `use f from mod lib/mylib` |

## The bot side

```dog
use read, write, exists, ls, history, send, send_id, edit, delete, sleep from mod bot;
```

* `$message`, `$sender`, `$reply`, `$reply_from` hold the message, as globals;
  `$message_id` and `$reply_id` are their ids (or null), for `get_message`,
  `download` and `image`.
* `history` rows have `id`, `kind` (`text`, `sticker`, `photo`, …), `media`,
  `reply_to`, `edited`, `reactions`, `tag` and more; the README lists them.
* `read path` raises when the file is missing: `try { read f } catch e ""`.
* `write path text` **doesn't create directories**. Make them first, with
  `/mkdir` or over SFTP.
* Messages starting with `/` are bot commands and never reach your script. Use
  another prefix, like `!help`, for your own commands.
* `main.dog` runs with the permissions of the person who sent the message, so
  it has to be readable and executable *by them*. In a group that means
  `/chmod g+rx main.dog` (`/init` sets this up for you).

The [README](../README.md#writing-scripts) lists everything in `mod bot`, and
[`examples/`](../examples/) has bots worth copying.
