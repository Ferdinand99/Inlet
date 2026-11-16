# The settings tabs: My profile, Appearance, Messages and media, Privacy, Advanced and
# Accounts. (Desktop is in desktop.ftl, Notifications in notifications.ftl.) The words
# follow Fluxer's own settings.

settings-save-failed = Could not save the setting. { $reason }
settings-not-connected = Inlet is not connected to an account right now.
settings-image-too-large = That picture is larger than 10 MB.
settings-image-unreadable = The picture could not be read: { $error }
settings-image-unsupported = Pick a PNG, JPEG, GIF, WebP or AVIF picture.

## My profile

settings-profile-title = My profile
settings-profile-loading = Loading your profile…
settings-profile-load-failed = Your profile could not be loaded. { $reason }
settings-profile-retry = Try again
settings-profile-picture = PICTURE
settings-profile-avatar-change = Change picture
settings-profile-avatar-remove = Remove picture
settings-profile-username = Your username is { $username }. It is changed in Fluxer.
settings-profile-name = Display name
settings-profile-name-hint = Your display name
settings-profile-name-note = Empty shows your username instead.
settings-profile-pronouns = Pronouns
settings-profile-pronouns-hint = For example they/them
settings-profile-bio = About me
settings-profile-bio-hint = A few words about you
settings-profile-bio-count = { $typed }/{ $limit }
settings-profile-bio-note = Markdown works in the bio. Shift+Enter starts a new line. Your plan decides how long it may be.
settings-profile-accent = Accent colour
settings-profile-accent-hint = For example #413cdd
settings-profile-accent-bad = That is not a colour. Write it as #rrggbb, or leave it empty for none.
settings-profile-accent-note = The colour of your profile card. Empty uses the colour of your banner.
settings-profile-banner = BANNER
settings-profile-banner-set = You have a profile banner.
settings-profile-banner-none = You have no profile banner.
settings-profile-banner-change = Change banner
settings-profile-banner-remove = Remove banner
settings-profile-banner-note = Pictures and animated GIFs work as a banner.
settings-profile-banner-locked = A banner needs a plan that includes it. A banner you already have can be removed.
settings-profile-will-remove = The picture is removed when you save.
settings-profile-will-use = { $file } is used when you save.
settings-profile-pick-prompt = Use this picture
settings-profile-save = Save changes
settings-profile-saving = Saving…
settings-profile-reset = Reset
settings-profile-save-failed = The profile was not saved. { $reason }

## Appearance

settings-appearance-title = Appearance
settings-appearance-theme = THEME
settings-appearance-theme-dark = Dark
settings-appearance-theme-coal = Coal
settings-appearance-theme-light = Light
settings-appearance-theme-system = System
settings-appearance-theme-sync = Sync theme across devices
settings-appearance-theme-sync-note = Theme changes sync to all your devices. Off keeps this device on its own theme. System follows the light or dark setting of this computer.
settings-appearance-messages = MESSAGES
settings-appearance-preview-name = Preview User
settings-appearance-preview-time-1 = 10:00 AM
settings-appearance-preview-time-2 = 10:01 AM
settings-appearance-preview-time-3 = 10:11 AM
settings-appearance-preview-1 = This is how messages appear
settings-appearance-preview-2 = With different display modes available
settings-appearance-preview-3 = Waiting for you to turn dense mode on. Nice!
settings-appearance-font-size = CHAT FONT SIZE
settings-appearance-px = { $size }px
settings-appearance-reset-font = Reset font size
settings-appearance-spacing = SPACE BETWEEN MESSAGE GROUPS
settings-appearance-reset-spacing = Reset spacing
settings-appearance-display = MESSAGE DISPLAY MODE
settings-appearance-cozy = Comfy
settings-appearance-cozy-hint = Spacious layout with clear visual separation between messages.
settings-appearance-compact = Dense
settings-appearance-compact-hint = Maximizes visible messages with minimal spacing.
settings-appearance-hide-avatars = Hide user avatars
settings-appearance-zoom = APP ZOOM
settings-appearance-reset-zoom = Reset zoom
settings-appearance-zoom-note = Zoom belongs to this computer, not to your account. The zoom keys change it from anywhere in Inlet.

## Messages and media

settings-media-title = Messages and media
settings-media-animations = ANIMATION
settings-media-emoji = Play animated emojis
settings-media-gif = Automatically play GIFs
settings-media-gif-note = Automatically play GIFs when Inlet is focused. With this off, a GIF or animated emoji plays while the pointer is over it.
settings-media-stickers = STICKER ANIMATION PREFERENCE
settings-media-stickers-always = Always animate
settings-media-stickers-always-hint = Stickers will always animate
settings-media-stickers-hover = Animate on interaction
settings-media-stickers-hover-hint = Stickers will animate when you hover or interact with them
settings-media-stickers-never = Never animate
settings-media-stickers-never-hint = Stickers will never animate
settings-media-inline = MEDIA DISPLAY
settings-media-inline-links = When posted as links in chat
settings-media-inline-uploads = When uploaded directly to chat
settings-media-links = LINK PREVIEWS
settings-media-embeds = Show embeds and preview website links
settings-media-embeds-note = Previews are fetched by the instance, never from the site itself.
settings-media-reactions-heading = REACTIONS
settings-media-reactions = Show emoji reactions on messages
settings-media-input = INPUT
settings-media-emoticons = Turn text smileys into emoji
settings-media-spoilers = SPOILER CONTENT
settings-media-spoilers-click = On click
settings-media-spoilers-click-hint = Show spoiler content when clicked
settings-media-spoilers-moderator = In channels I moderate
settings-media-spoilers-moderator-hint = Always show spoiler content in channels where you have the "Manage Messages" permission
settings-media-spoilers-always = Always
settings-media-spoilers-always-hint = Always show spoiler content

## Privacy

settings-privacy-title = Privacy
settings-privacy-friends = Friend requests
settings-privacy-friends-hint = Choose who can send you a friend request.
settings-privacy-friends-anyone = Everyone
settings-privacy-friends-anyone-hint = Anyone who finds you can send a request. Turn this off to pick groups below.
settings-privacy-friends-of-friends = Friends of friends
settings-privacy-friends-of-friends-hint = People who share at least one friend with you.
settings-privacy-friends-servers = Community members
settings-privacy-friends-servers-hint = People who share a community with you.
settings-privacy-dms = Direct messages
settings-privacy-dms-hint = Friends can always message you. These settings cover people and bots you only share a community with.
settings-privacy-dms-from-members = Allow direct messages from community members
settings-privacy-dms-note = Applies to communities you join from now on. You can also change it for each community.
settings-privacy-dms-from-bots = Allow direct messages from community bots
settings-privacy-dms-bots-note = Applies to bots in communities you join from now on. You can also change it for each community.
settings-privacy-ask-members-allow = Allow direct messages from community members?
settings-privacy-ask-members-block = Block direct messages from community members?
settings-privacy-ask-bots-allow = Allow bots to send you direct messages?
settings-privacy-ask-bots-block = Block bots from sending you direct messages?
settings-privacy-ask-body = Do you also want this for the communities you are already in? You can also change it per community in Fluxer.
settings-privacy-ask-allow-all = Allow for all communities
settings-privacy-ask-block-all = Block for all communities
settings-privacy-ask-skip = Skip this step
settings-privacy-ask-cancel = Cancel
settings-privacy-calls = Incoming calls
settings-privacy-calls-hint = Choose who can start a call with you.
settings-privacy-calls-nobody = Block all incoming calls
settings-privacy-calls-friends = Only friends can call you (recommended)
settings-privacy-calls-custom = Friends and selected groups can call you
settings-privacy-calls-everyone = Anyone can call you, including strangers
settings-privacy-calls-silent = Receive calls silently
settings-privacy-calls-silent-hint = Calls you allow still come through, but they don't ring.
settings-privacy-groups = Group chats
settings-privacy-groups-hint = Choose who can add you to a group chat without asking first.
settings-privacy-groups-nobody = No one can add you to group chats without asking
settings-privacy-groups-friends = Only friends can add you without asking (recommended)
settings-privacy-groups-custom = Friends and selected groups can add you without asking
settings-privacy-groups-everyone = Anyone can add you to group chats
settings-privacy-profile = Who can see your full profile
settings-privacy-profile-all = Friends and all communities
settings-privacy-profile-all-hint = Your full profile is visible to friends and to anyone in your communities
settings-privacy-profile-small = Friends and small communities only
settings-privacy-profile-small-hint = Your full profile is visible to friends and members of your communities with 200 or fewer members
settings-privacy-profile-friends = Friends only
settings-privacy-profile-friends-hint = Your full profile is only visible to your friends
settings-privacy-sensitive = Sensitive content
settings-privacy-sensitive-hint = What to do with media that may not be suitable for everyone.
settings-privacy-sensitive-show = Show
settings-privacy-sensitive-blur = Blur
settings-privacy-sensitive-block = Block
settings-privacy-sensitive-friends = Direct messages from friends
settings-privacy-sensitive-friends-hint = Media your friends send you in direct messages.
settings-privacy-sensitive-others = Direct messages from others
settings-privacy-sensitive-others-hint = Media in group chats and in direct messages from people who aren't your friends.
settings-privacy-sensitive-servers = Messages in community channels
settings-privacy-sensitive-servers-hint = Media in channels that are not marked 18+. Channels marked 18+ always show it.

## Advanced

settings-advanced-title = Advanced
settings-advanced-developer = DEVELOPER
settings-advanced-developer-mode = Developer mode
settings-advanced-developer-note = Adds Copy ID to the menus of messages and people.
settings-advanced-id = YOUR USER ID
settings-advanced-copy-id = Copy ID

## Accounts

settings-accounts-title = Accounts
settings-accounts-signed-in = SIGNED IN
settings-accounts-shown = Shown now
settings-accounts-switch = Switch to this account
settings-accounts-sign-out = Sign out
settings-accounts-note = Inlet can be signed in to up to ten accounts, on one instance or several. Accounts you are not looking at stay quiet in the background and still notify you.
