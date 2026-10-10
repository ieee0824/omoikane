"""Parse CLDR number affixes without discarding quoted literals or negative patterns."""

NUMBER_CHARACTERS = frozenset("#0@,.123456789E")


def split_subpatterns(pattern):
    """Return one or two subpatterns; semicolons inside quotes remain literals."""
    quoted = False
    start = 0
    parts = []
    index = 0
    while index < len(pattern):
        char = pattern[index]
        if char == "'":
            if index + 1 < len(pattern) and pattern[index + 1] == "'":
                index += 2
                continue
            quoted = not quoted
        elif char == ";" and not quoted:
            parts.append(pattern[start:index])
            start = index + 1
        index += 1
    if quoted:
        raise ValueError(f"unclosed quote in number pattern: {pattern!r}")
    parts.append(pattern[start:])
    if len(parts) not in (1, 2) or any(not part for part in parts):
        raise ValueError(f"invalid number subpatterns: {pattern!r}")
    return parts


def parse_subpattern(pattern):
    """Return owned token dictionaries, preserving affix punctuation and bidi controls."""
    tokens = []
    literal = []
    quoted = False
    index = 0

    def flush():
        if literal:
            tokens.append({"type": "literal", "value": "".join(literal)})
            literal.clear()

    while index < len(pattern):
        char = pattern[index]
        if char == "'":
            if index + 1 < len(pattern) and pattern[index + 1] == "'":
                literal.append("'")
                index += 2
                continue
            quoted = not quoted
            index += 1
            continue
        if quoted:
            literal.append(char)
            index += 1
            continue
        if char in "#0@":
            flush()
            start = index
            while index < len(pattern):
                if pattern[index] in NUMBER_CHARACTERS:
                    index += 1
                elif pattern[index] in "+-" and pattern[index - 1] == "E":
                    index += 1
                else:
                    break
            tokens.append({"type": "number", "pattern": pattern[start:index]})
            continue
        if char == "¤":
            flush()
            start = index
            while index < len(pattern) and pattern[index] == "¤":
                index += 1
            tokens.append({"type": "currency", "width": index - start})
            continue
        part_type = {"-": "minusSign", "+": "plusSign", "%": "percentSign", "‰": "perMille"}.get(char)
        if part_type:
            flush()
            tokens.append({"type": part_type})
        else:
            literal.append(char)
        index += 1
    flush()
    if quoted:
        raise ValueError(f"unclosed quote in subpattern: {pattern!r}")
    if sum(token["type"] == "number" for token in tokens) != 1:
        raise ValueError(f"expected one numeric portion: {pattern!r}")
    return tokens


def parse_number_pattern(pattern):
    parts = split_subpatterns(pattern)
    positive = parse_subpattern(parts[0])
    negative = parse_subpattern(parts[1]) if len(parts) == 2 else [{"type": "minusSign"}, *positive]
    return {"raw": pattern, "positive": positive, "negative": negative, "explicitNegative": len(parts) == 2}
