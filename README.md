# vban-common

Wire types and parser for the VBAN protocol. No sockets, no threads, no I/O.

VBAN is VB-Audio's audio over UDP protocol. One packet layout carries four
sub-protocols, selected by the top three bits of the first format byte.

## What it covers

- The 28 byte header, read and written
- TEXT requests, the control channel: `Strip[0].Gain=-6.0;Bus[1].Mute=1`
- SERVICE packets: ping, pong, and RT subscription
- The RT state packet, 1412 bytes at fixed offsets

AUDIO and SERIAL are recognised and declined rather than parsed.

## What it does not do

It knows that `Strip[0].Gain` addresses field `gain` of strip 0. It does not
know what a gain is, whether it is decibels, or what changing it should do.
That belongs to the application. Keeping the split is what lets the tests run
against nothing at all.

## License

Public domain. See UNLICENSE.
