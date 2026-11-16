// SPDX-License-Identifier: AGPL-3.0-or-later
//! The settings tabs, one file each. A task that adds a tab adds its file here and one
//! line to [`all`].

mod accounts;
mod advanced;
mod appearance;
mod desktop;
mod messages_media;
mod my_profile;
mod notifications;
mod privacy;

use super::SettingsTab;

/// Every tab, in the order the list shows them.
pub fn all() -> Vec<Box<dyn SettingsTab>> {
    vec![
        Box::new(my_profile::MyProfileTab),
        Box::new(appearance::AppearanceTab),
        Box::new(messages_media::MessagesMediaTab),
        Box::new(privacy::PrivacyTab),
        Box::new(notifications::NotificationsTab),
        Box::new(desktop::DesktopTab),
        Box::new(advanced::AdvancedTab),
        Box::new(accounts::AccountsTab),
    ]
}
