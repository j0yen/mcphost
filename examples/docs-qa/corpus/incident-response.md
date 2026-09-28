# Incident response runbook

This page covers how Acorn Robotics classifies and responds to production
incidents, from detection to postmortem.

## Severity levels

Sev1 is a full outage or a safety-relevant failure in shipped hardware;
Sev2 is a major feature degraded for many customers; Sev3 is a minor,
workaround-available issue. Severity is set by whoever declares the
incident and can be raised or lowered as more information comes in, but
never lowered without the incident commander's sign-off.

## Who gets paged first

For a Sev1 incident, the on-call engineer is paged first, automatically,
through PagerDuty, regardless of the time of day. The on-call engineer has
fifteen minutes to acknowledge before the page escalates to the secondary
on-call, and thirty minutes before it escalates to the engineering
director. Sev2 pages the on-call engineer with a slower escalation clock
(one hour instead of fifteen minutes); Sev3 does not page anyone and is
picked up during normal working hours.

## Declaring an incident

Anyone can declare an incident by posting in #incidents with the suspected
severity and a one-line description. Declaring immediately opens an
incident channel, starts a timeline document, and pages according to the
severity rules above. It is always acceptable to declare and later
downgrade; it is not acceptable to wait for certainty before declaring a
likely Sev1.

## Incident commander

The first on-call engineer to acknowledge becomes incident commander
unless a more senior engineer explicitly takes over. The incident
commander's job is coordinating, not necessarily fixing: delegating
investigation, deciding when to escalate further, and deciding when the
incident is resolved. The incident commander writes the first draft of
the timeline in real time, not from memory afterward.

## Communication

Customer-facing status updates go through the status page, owned by
support, updated at least every thirty minutes during a Sev1 regardless of
whether there is new information ("still investigating" is an acceptable
update). Internal updates go in the incident channel; do not use direct
messages for incident coordination, since the whole point of the channel
is that anyone can catch up without asking.

## Resolution and postmortem

An incident is resolved when the customer-visible symptom stops, not when
the root cause is fully understood. Every Sev1 and Sev2 gets a postmortem
within five business days: a timeline, a root cause, contributing factors,
and follow-up action items with owners and due dates. Postmortems are
blameless by policy; a postmortem that names an individual as the cause
rather than a process gap is sent back for a rewrite.

## Common questions

**Who should be paged first for a Sev1 incident?** The on-call engineer,
automatically, via PagerDuty.

**Can a Sev3 incident page someone at night?** No, Sev3 never pages;
it is picked up during normal working hours.

**How soon does a Sev1 need a postmortem?** Within five business days of
resolution.
