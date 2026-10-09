# Bomb Code for iPhone

A companion to the desktop app: see every thread on your Mac and your servers, read along live, answer
approvals, send follow-ups and start new threads. Design: `docs/plan/ios_companion_plan.md`.

The UI is SwiftUI. Everything under it (pairing, pinned TLS, the protocol, the transcript reducer) is the
Rust crate `crates/bomb_mobile`, built into `BombMobile.xcframework` with generated Swift bindings.

## Build

    scripts/build-ios.sh            # release; `debug` for a faster build
    open ios/BombCode.xcodeproj

Run the script again whenever `crates/bomb_mobile` (or what it uses) changes. Both outputs are gitignored.

In Xcode, set your team under Signing & Capabilities and change the bundle id
(`com.bombcode.companion`) to one in your account. The app needs to be signed even in the simulator:
an unsigned build can't use the Keychain, so pairings would be forgotten on quit.

## Try it without spending tokens

A local `bombd` with the mock agent, and a tap-free run in the simulator (debug builds only):

    cargo build -p bomb_server
    HOME=/tmp/bombd-phone BOMB_SMOKE=1 ./target/debug/bombd up --data /tmp/bombd-phone/gateway \
        --listen 127.0.0.1:17443 --public 127.0.0.1:17443        # prints a bomb://pair link
    xcodebuild -project ios/BombCode.xcodeproj -scheme BombCode -destination 'generic/platform=iOS Simulator' \
        -derivedDataPath target/ios-derived CODE_SIGN_IDENTITY=- build
    xcrun simctl install booted target/ios-derived/Build/Products/Debug-iphonesimulator/BombCode.app
    SIMCTL_CHILD_BOMB_SMOKE=1 SIMCTL_CHILD_BOMB_PAIR='bomb://pair?…' xcrun simctl launch booted com.bombcode.companion

It pairs, starts a mock thread in a `smoke` project and opens it. Add `SIMCTL_CHILD_BOMB_SMOKE_LIST=1`
to stay on the thread list.

## Reaching your machines

The phone dials the `host:port` in the pairing link directly; there is no relay. Away from home, put the
phone, the Mac and any server on the same Tailscale network and pair using their Tailscale addresses.
