# API rate limits

This page covers the rate limits on Acorn Robotics's public telemetry API,
used by partners to pull sensor data off deployed hardware.

## Default limits

The default plan allows sixty requests per minute per API key, with a
burst allowance of ten additional requests absorbed by a token bucket that
refills at one token per second. Exceeding the limit returns HTTP 429 with
a `Retry-After` header naming the number of seconds to wait before the
next request will succeed.

## Raising your limit

Partners on the enterprise plan get six hundred requests per minute by
default and can request a further increase through their account manager,
who reviews usage patterns before approving. Sudden, unexplained traffic
spikes may be rate-limited more aggressively regardless of plan while the
security team investigates, then restored once cleared.

## Per-endpoint limits

The bulk export endpoint has its own, stricter limit of five requests per
hour per API key, independent of the general limit above, because a single
bulk export can already return the equivalent of thousands of ordinary
requests worth of data. The streaming endpoint is not rate-limited by
request count but is capped at one open connection per API key.

## Monitoring your usage

Every response carries `X-RateLimit-Remaining` and `X-RateLimit-Reset`
headers so a client can back off before hitting 429 rather than after.
The partner dashboard also shows a rolling seven-day usage graph, updated
hourly, broken down by endpoint.

## Retry guidance

On a 429, respect the `Retry-After` header exactly rather than guessing;
retrying immediately after a 429 extends the penalty window under the
current rate limiter implementation. Exponential backoff with jitter is
recommended for any other 5xx response, but not for 429, which already
tells you precisely how long to wait.

## Common questions

**What is the default rate limit?** Sixty requests per minute per API
key, with a small burst allowance.

**Is the bulk export endpoint covered by the general rate limit?** No, it
has its own stricter limit of five requests per hour.

**What should I do when I get a 429?** Wait exactly as long as the
`Retry-After` header says before retrying.
