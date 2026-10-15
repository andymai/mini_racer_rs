#!/usr/bin/env python3
# PreToolUse (Bash): refuses commands that skip the git hooks. Exit 2 blocks the
# call and shows the message to the agent. This is a tripwire against an agent
# skipping hooks by habit, not a security boundary: CI reruns every check.
#
# The command is tokenized like a shell would (quotes, &&, |, ;) so flags are
# judged where git would see them, not wherever they appear in the text.
import json
import sys

OPERATOR_CHARS = set(";&|()\n")
GIT_GLOBAL_WITH_VALUE = {"-C", "-c", "--git-dir", "--work-tree", "--namespace", "--config-env", "--exec-path"}
COMMIT_SHORT_WITH_VALUE = set("mFCct")
COMMIT_SHORT_OPTIONAL_ATTACHED = set("Su")
COMMIT_LONG_WITH_VALUE = {
    "--message", "--file", "--author", "--date", "--reuse-message", "--reedit-message",
    "--fixup", "--squash", "--template", "--cleanup", "--trailer", "--pathspec-from-file",
}
SHELLS = {"bash", "sh", "zsh", "dash", "ksh"}


# A minimal shell lexer: adjacent quoted and unquoted text form one word, quoted
# punctuation stays text, and unquoted ; & | ( ) and newlines separate commands.
# Redirections (> < >& and friends) are skipped rather than treated as separators.
def words_and_separators(command):
    out, word, started, i = [], [], False, 0
    n = len(command)

    def flush():
        nonlocal word, started
        if started:
            out.append(("word", "".join(word)))
        word, started = [], False

    while i < n:
        c = command[i]
        if c == "\\" and i + 1 < n:
            if command[i + 1] != "\n":
                word.append(command[i + 1])
                started = True
            i += 2
        elif c == "'":
            end = command.find("'", i + 1)
            end = n if end < 0 else end
            word.append(command[i + 1:end])
            started, i = True, end + 1
        elif c == '"':
            i += 1
            while i < n and command[i] != '"':
                if command[i] == "\\" and i + 1 < n and command[i + 1] in '"\\$`':
                    i += 1
                word.append(command[i])
                i += 1
            started, i = True, i + 1
        elif c in " \t\r":
            flush()
            i += 1
        elif c == "#" and not started:
            end = command.find("\n", i)
            i = n if end < 0 else end
        elif c in OPERATOR_CHARS or c in "<>":
            flush()
            j = i
            while j < n and (command[j] in OPERATOR_CHARS or command[j] in "<>"):
                j += 1
            run = command[i:j]
            out.append(("redirect" if any(ch in run for ch in "<>") else "separator", run))
            i = j
        else:
            word.append(c)
            started = True
            i += 1
    flush()
    return out


# The shell runs $(...) and `...` even inside double quotes, so their contents
# are checked as commands of their own; single quotes and escapes keep them literal.
def substitutions(command):
    found, i, n, double = [], 0, len(command), False
    while i < n:
        if command[i] == "\\":
            i += 2
        elif command[i] == '"':
            double = not double
            i += 1
        elif command[i] == "'" and not double:
            end = command.find("'", i + 1)
            i = n if end < 0 else end + 1
        elif command.startswith("$(", i):
            depth, j = 1, i + 2
            while j < n and depth:
                depth += {"(": 1, ")": -1}.get(command[j], 0)
                j += 1
            found.append(command[i + 2:j - 1])
            i = j
        elif command[i] == "`":
            end = command.find("`", i + 1)
            end = n if end < 0 else end
            found.append(command[i + 1:end])
            i = end + 1
        else:
            i += 1
    return found


def simple_commands(command):
    current = []
    for kind, text in words_and_separators(command):
        if kind == "separator":
            if current:
                yield current
            current = []
        elif kind == "word":
            current.append(text)
    if current:
        yield current


def no_verify(arg):
    # git accepts unique prefixes of long options; --no-veri is the shortest one
    # that cannot also mean --no-verbose.
    return len(arg) >= len("--no-veri") and "--no-verify".startswith(arg)


def hooks_path(text):
    return "core.hookspath" in text.lower()


def assignment(word):
    name, sep, value = word.partition("=")
    return (name, value) if sep and name.isidentifier() else None


def check_commit(args):
    i = 0
    while i < len(args):
        arg = args[i]
        if arg == "--":
            return None
        if arg.startswith("--"):
            if no_verify(arg.split("=", 1)[0]):
                return "--no-verify"
            if arg in COMMIT_LONG_WITH_VALUE:
                i += 1
        elif arg.startswith("-") and len(arg) > 1:
            cluster = arg[1:]
            for position, flag in enumerate(cluster):
                if flag == "n":
                    return "git commit -n"
                if flag in COMMIT_SHORT_WITH_VALUE:
                    if position == len(cluster) - 1:
                        i += 1
                    break
                if flag in COMMIT_SHORT_OPTIONAL_ATTACHED:
                    break
        i += 1
    return None


def check_git(words):
    i = 1
    while i < len(words) and words[i].startswith("-"):
        option = words[i]
        if option in ("-c", "--config-env") and i + 1 < len(words) and hooks_path(words[i + 1]):
            return "core.hooksPath"
        if option.startswith("--config-env=") and hooks_path(option):
            return "core.hooksPath"
        i += 2 if option in GIT_GLOBAL_WITH_VALUE else 1
    if i >= len(words):
        return None
    subcommand, args = words[i], words[i + 1:]
    if subcommand == "commit":
        return check_commit(args)
    if subcommand == "config":
        positional = [a for a in args if not a.startswith("-")]
        if positional and positional[0] in ("get", "list", "unset"):
            return None
        setting = positional[1:] if positional and positional[0] == "set" else positional
        if setting and hooks_path(setting[0]) and (len(setting) >= 2 or "--add" in args or "--replace-all" in args):
            return "core.hooksPath"
        return None
    for arg in args:
        if arg == "--":
            break
        if no_verify(arg.split("=", 1)[0]):
            return "--no-verify"
    return None


def check(command, depth=0):
    if depth > 3:
        return None
    for inner in substitutions(command):
        reason = check(inner, depth + 1)
        if reason:
            return reason
    for words in simple_commands(command):
        env = {}
        while words and assignment(words[0]):
            name, value = assignment(words[0])
            env[name] = value
            words = words[1:]
        if words and words[0] == "env":
            words = words[1:]
            while words and (words[0].startswith("-") or assignment(words[0])):
                if assignment(words[0]):
                    name, value = assignment(words[0])
                    env[name] = value
                words = words[1:]
        if any(name.startswith("GIT_CONFIG") and hooks_path(value) for name, value in env.items()):
            return "core.hooksPath"
        if not words:
            continue
        program = words[0].rsplit("/", 1)[-1]
        if program == "export" and any(assignment(w) == ("HUSKY", "0") for w in words[1:]):
            return "HUSKY=0"
        if program == "git":
            if env.get("HUSKY") == "0":
                return "HUSKY=0"
            reason = check_git(words)
            if reason:
                return reason
        if program in SHELLS:
            for position, word in enumerate(words[1:-1], start=1):
                if word.startswith("-") and not word.startswith("--") and "c" in word:
                    reason = check(words[position + 1], depth + 1)
                    if reason:
                        return reason
        if program == "eval":
            reason = check(" ".join(words[1:]), depth + 1)
            if reason:
                return reason
    return None


def main():
    command = json.load(sys.stdin).get("tool_input", {}).get("command") or ""
    reason = check(command)
    if reason:
        print(
            f"Blocked ({reason}): this command skips the git hooks. Fix what the hook "
            "reports instead; see .claude/skills/pr-workflow/SKILL.md.",
            file=sys.stderr,
        )
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
