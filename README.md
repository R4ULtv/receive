# Receive

A small desktop app for managing email across multiple domains on [Resend](https://resend.com).

**This is very much a side project.** I built it because I have several domains on Resend and wanted a simple place to read and reply to their emails. It's built around my own workflow, so expect a small feature set and some rough edges.

## What it does

- Brings your inbox and sent mail into one app, with filters for each domain.
- Lets you write and reply to emails, with a draft saved as you go.
- Shows delivery updates, such as delivered, delayed, or bounced.
- Saves downloaded messages on your computer and lets you search them.
- Checks for new mail while the app is open.

It connects directly to Resend. There's no separate server to set up.

## Try it

Receive is shared as source code. Install [Rust](https://rustup.rs), plus the tools for your device:

- **Windows:** Visual Studio Build Tools with **Desktop development with C++** and a Windows SDK.
- **Mac:** macOS 15 or later and Xcode Command Line Tools, which you can install with `xcode-select --install` in Terminal. Apple Silicon meets the [UI framework's requirements](https://github.com/longbridge/gpui-kit/blob/main/website/docs/installation.md).

Windows has been tested. The project is configured for macOS too, but still needs to be built and tried on a Mac.

From the project folder, run:

```sh
cargo run
```

The first run takes a while to compile. To look around with sample messages without connecting an account:

```sh
cargo run -- --preview
```

### Mac app icon

The Receive icon is included for the Dock when you use `cargo run`. To get a local app you can open from Finder or drag to your Dock, run this on your Mac:

```sh
bash tools/macos-app.sh
open target/Receive.app
```

This creates a local `Receive.app` using the normal development build. Run the script again after updating the source. The Mac icon still needs visual testing on a Mac.

## Connect your account

1. Open **Settings > Connect Resend** and sign in through your browser, or enter a **Full access** Resend API key.
2. Make sure receiving is set up for your domains in Resend.
3. Use **Compose** to send from an address on a domain you've verified for sending.

You can manage multiple domains within one connected Resend account. If you're sending to several people, separate their addresses with semicolons.

## A few things to know

Emails are displayed as plain text. Attachment names are shown, but uploading and downloading attachments aren't supported yet. You can open a message in the Resend dashboard to see more.

Downloaded mail and drafts stay on your computer. The local mail archive isn't encrypted; login details are stored separately in your operating system's credential store. The app also fetches website icons for domains shown in the interface.

Keep Receive open to download and check for mail. It can keep messages it has already saved, but it can't recover emails that have expired from Resend before being downloaded.

You can use the same Resend account on your Windows PC and Mac; sign in separately on each device. Drafts, read/unread status, and downloaded archives are local to each device and don't sync between them.

Large mailboxes may be slow, and multiple simultaneous accounts aren't supported. If you sign in through your browser, run only one copy of Receive per device at a time.

For development checks and more detailed limitations, see the [review notes](REVIEW.md).

## Built with

Rust, [GPUI Kit](https://github.com/longbridge/gpui-kit), and Resend's official [resend-rs](https://resend.com/docs/send-with-rust) SDK.

## License

[MIT](LICENSE).
