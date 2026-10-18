"""Add required flags to DRG's Steam launch options in a localconfig.vdf.

usage: python3 drg_launch_opts.py LOCALCONFIG.VDF BACKUP_SUFFIX FLAG...

Edits only UserLocalConfigStore/Software/Valve/Steam/apps/548430/LaunchOptions, keeps any
existing options and only appends flags that are missing. Every other byte is left as is.
Steam must not be running, it rewrites this file on exit.
"""

import os
import sys

TARGET = ["userlocalconfigstore", "software", "valve", "steam", "apps", "548430"]


def tokens(text):
    """Yield (kind, start, end, value); kind is 's' (quoted string), '{' or '}'."""
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        if c.isspace():
            i += 1
        elif text.startswith("//", i):
            j = text.find("\n", i)
            i = n if j < 0 else j
        elif c in "{}":
            yield c, i, i + 1, None
            i += 1
        elif c == '"':
            j, buf = i + 1, []
            while j < n and text[j] != '"':
                if text[j] == "\\" and j + 1 < n:
                    j += 1
                buf.append(text[j])
                j += 1
            if j >= n:
                raise ValueError("unterminated string at offset %d" % i)
            yield "s", i, j + 1, "".join(buf)
            i = j + 1
        else:
            raise ValueError("unexpected %r at offset %d" % (c, i))


def locate(text):
    """Return (apps_open, block_open, value_span, value) as offsets just past '{' / token span."""
    stack, key = [], None
    apps_open = block_open = value_span = value = None
    for kind, start, end, val in tokens(text):
        if kind == "{":
            stack.append((key or "").lower())
            key = None
            if stack == TARGET[:5] and apps_open is None:
                apps_open = end
            elif stack == TARGET and block_open is None:
                block_open = end
        elif kind == "}":
            if not stack:
                raise ValueError("unbalanced '}' at offset %d" % start)
            stack.pop()
            key = None
        elif key is None:
            key = val
        else:
            if stack == TARGET and key.lower() == "launchoptions" and value_span is None:
                value_span, value = (start, end), val
            key = None
    if stack:
        raise ValueError("unbalanced '{'")
    return apps_open, block_open, value_span, value


def esc(s):
    return s.replace("\\", "\\\\").replace('"', '\\"')


def main():
    path, suffix, flags = sys.argv[1], sys.argv[2], sys.argv[3:]
    with open(path, "rb") as f:
        raw = f.read()
    text = raw.decode("utf-8", "surrogateescape")
    apps_open, block_open, value_span, current = locate(text)

    current = current or ""
    have = {t.lower() for t in current.split()}
    missing = [f for f in flags if f.lower() not in have]
    if not missing:
        print("  already set: %s" % current)
        return
    new = (current.rstrip() + " " if current.strip() else "") + " ".join(missing)

    if value_span:
        text = text[: value_span[0]] + '"%s"' % esc(new) + text[value_span[1] :]
    elif block_open is not None:
        ins = '\n%s"LaunchOptions"\t\t"%s"' % ("\t" * 6, esc(new))
        text = text[:block_open] + ins + text[block_open:]
    elif apps_open is not None:
        t5, t6 = "\t" * 5, "\t" * 6
        ins = '\n%s"548430"\n%s{\n%s"LaunchOptions"\t\t"%s"\n%s}' % (t5, t5, t6, esc(new), t5)
        text = text[:apps_open] + ins + text[apps_open:]
    else:
        sys.exit("  no Software/Valve/Steam/apps section found, not touching this file")

    out = text.encode("utf-8", "surrogateescape")
    locate(out.decode("utf-8", "surrogateescape"))  # re-parse: never write a file we broke
    with open(path + suffix, "wb") as f:
        f.write(raw)
    tmp = path + ".tmp-mint"
    with open(tmp, "wb") as f:
        f.write(out)
    os.replace(tmp, path)
    print("  %r -> %r (backup: %s)" % (current, new, os.path.basename(path + suffix)))


if __name__ == "__main__":
    try:
        main()
    except ValueError as e:
        sys.exit("  could not parse: %s" % e)
