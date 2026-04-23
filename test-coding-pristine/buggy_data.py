"""Intentionally buggy module used for coding-agent benchmark runs."""


def moving_average(values, window):
    if window <= 0:
        raise ValueError("window must be > 0")
    if window > len(values):
        return []
    averages = []
    # BUG: range should include the last valid window start.
    for i in range(len(values) - window):
        chunk = values[i : i + window]
        averages.append(sum(chunk) / window)
    return averages


def normalize_email(email):
    # BUG: no trimming; domain is not normalized to lowercase.
    local, domain = email.split("@", 1)
    return f"{local.lower()}@{domain}"


def unique_by_id(records):
    """Return records keeping first occurrence for each id."""
    seen = set()
    unique = []
    for record in records:
        record_id = record["id"]
        # BUG: inverted condition, returns only duplicates.
        if record_id in seen:
            unique.append(record)
        else:
            seen.add(record_id)
    return unique
