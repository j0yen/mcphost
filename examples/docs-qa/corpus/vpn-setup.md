# VPN setup

Acorn Robotics runs a WireGuard VPN for access to internal tools that are
not exposed to the public internet: the build server, the internal wiki
search index, and the lab equipment dashboards.

## Installing the client

Install the WireGuard client for your platform from the official
WireGuard site, not from a third-party mirror. Do not use a generic
OpenVPN client; this network does not run OpenVPN, and a mismatched
client will simply time out with no useful error message.

## Getting your configuration

Request a VPN configuration file from the #it-requests channel, naming
your device (laptop, desk workstation, and so on -- each device needs its
own configuration, since keys are per-device, not per-person). IT
generates a `.conf` file and a QR code; import either into the WireGuard
client. The QR code is the faster path for a phone or tablet.

## Connecting

The VPN client connects on port 51820, the standard WireGuard UDP port.
If your home router or hotel network blocks outbound UDP entirely, the
connection will hang at "handshake" and never complete; switching to a
phone hotspot is the usual workaround while traveling. Corporate laptops
ship with a firewall exception for port 51820 already configured, so this
mainly affects home networks with aggressive default firewalls.

## Split tunneling

By default, only traffic to internal ranges routes through the VPN; your
general internet traffic (streaming, personal browsing) does not slow
down while connected. If you need full-tunnel mode for a specific
security review, request it explicitly in #it-requests -- it is not the
default because it doubles support tickets about "the internet feels
slow" during audits.

## Troubleshooting

If the handshake never completes, check that outbound UDP on port 51820
is not blocked, that your system clock is correct (WireGuard's handshake
is time-sensitive), and that you imported the most recent configuration
file -- old configurations are revoked automatically ninety days after
issue and silently fail rather than erroring clearly.

## Revocation

VPN access is revoked automatically when your account is disabled, and
manually when a laptop is reported lost or stolen. Report a lost device to
#it-requests immediately; revocation is usually completed within fifteen
minutes of the report during business hours.

## Common questions

**What port does the VPN client connect on?** Port 51820, over UDP, which
is the standard WireGuard port.

**Can I use one configuration file on two devices?** No, each device
needs its own configuration; sharing a key between devices causes random
disconnects as the two devices fight over the same handshake state.

**Does the VPN slow down my personal browsing?** No, split tunneling means
only internal-range traffic uses the tunnel by default.
