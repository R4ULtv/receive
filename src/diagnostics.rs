use crate::{api::Api, auth::Credentials, model::Folder};
use anyhow::{Context, Result};

/// Report Resend's required public DNS records without changing the domain.
pub fn domain(name: &str) -> Result<()> {
    let credentials = Credentials::load()?
        .context("No saved Resend connection was found. Connect an API key in Settings first.")?;
    let mut api = Api::new(credentials)?;
    let domain = api.domain_dns(name)?;
    // Limit output to the domain configuration, never credentials or mail.
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "name": domain["name"],
            "status": domain["status"],
            "region": domain["region"],
            "capabilities": domain["capabilities"],
            "records": domain["records"],
        }))?
    );
    Ok(())
}

/// Exercise only read endpoints; report counts and errors, never credentials or mail content.
pub fn run() -> Result<()> {
    let credentials = Credentials::load()?
        .context("No saved Resend connection was found. Connect an API key in Settings first.")?;
    println!("Saved connection found in the operating system credential store.");
    let mut api = Api::new(credentials)?;
    let mut failures = 0;
    for folder in [Folder::Inbox, Folder::Sent] {
        match api.list(folder, None) {
            Ok(page) => {
                println!(
                    "{} listing: OK ({} messages on first page, has_more={})",
                    folder.name(),
                    page.data.len(),
                    page.has_more
                );
                if folder == Folder::Sent {
                    let mut statuses = std::collections::BTreeMap::new();
                    for email in &page.data {
                        *statuses.entry(email.delivery_status().label()).or_insert(0) += 1;
                    }
                    println!("Sent delivery statuses (first page): {statuses:?}");
                }
                if let Some(email) = page.data.first() {
                    match api.email(folder, &email.id) {
                        Ok(email) => {
                            println!("{} first message content: OK", folder.name());
                            if folder == Folder::Sent {
                                println!(
                                    "Sent first message status: {}",
                                    email.delivery_status().label()
                                );
                            }
                        }
                        Err(error) => {
                            failures += 1;
                            eprintln!("{} message content: {error:#}", folder.name());
                        }
                    }
                }
            }
            Err(error) => {
                failures += 1;
                eprintln!("{} listing: {error:#}", folder.name());
            }
        }
    }
    anyhow::ensure!(failures == 0, "{failures} Resend read operation(s) failed.");
    Ok(())
}
