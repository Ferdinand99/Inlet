# Friends, friend requests, blocks, the DM list's actions and group DMs.

## The DM list

social-friends = Friends
social-personal-notes = Personal notes
social-pin-dm = Pin DM
social-unpin-dm = Unpin DM
social-close-dm = Close DM
social-group-settings = Group settings

## The friends page

social-tab-online = Online
social-tab-all = All
social-tab-pending = Pending
social-tab-blocked = Blocked
social-add-friend = Add Friend
social-search = Search
social-online-count = Online — { $count }
social-all-count = All friends — { $count }
social-pending-count = Pending — { $count }
social-blocked-count = Blocked — { $count }
social-empty-online = No friends are online right now.
social-empty-all = No friends yet. Add one with their username.
social-empty-pending = There are no pending friend requests.
social-empty-blocked = You have not blocked anyone.
social-incoming-request = Incoming friend request
social-outgoing-request = Outgoing friend request
social-blocked = Blocked
social-message = Message
social-accept-request = Accept Friend Request
social-cancel-request = Cancel Friend Request
social-unblock = Unblock

## Adding a friend

social-add-friend-body = You can add friends with their username.
social-add-friend-placeholder = Username, or username#0000
social-add-friend-send = Send Friend Request
social-add-friend-sent = Your friend request to { $name } was sent.
social-add-friend-invalid = Enter a username of up to 32 characters, with an optional #tag of 1-4 digits.
social-sending = Sending…

## Confirmations

social-remove-friend = Remove Friend
social-remove-friend-title = Remove { $name }?
social-remove-friend-body = Are you sure you want to remove { $name } from your friends?
social-block = Block
social-block-title = Block { $name }?
social-block-body = { $name } will be removed from your friends, and their messages will be hidden.
social-leave-group = Leave Group
social-leave-group-title = Leave { $name }?
social-leave-group-body = You will not be able to rejoin { $name } unless you are added again.

## Group DMs

social-new-dm-title = Select friends
social-new-dm-body = You can add { $count } more { $count ->
    [one] friend.
   *[other] friends.
}
social-new-dm-open = Open DM
social-new-group-create = Create Group DM
social-group-dm-search = Type the username of a friend
social-group-no-friends = No friends to add.
social-group-add-title = Add friends to { $name }
social-group-add-body = Pick the friends to add. You can add { $count } more.
social-group-full = This group is full. Remove someone before adding more people.
social-group-add = Add Friends
social-group-name = Group name
social-group-name-placeholder = Group name
social-group-icon-prompt = Choose a group icon
social-group-icon = Group icon
social-group-icon-upload = Upload icon
social-group-icon-change = Change icon
social-group-icon-remove = Remove icon
social-group-icon-removing = The icon will be removed when you save.
social-group-icon-chosen = New icon: { $file }
social-edit-group = Edit group
social-group-updated = Group updated
social-group-mature = Mature content
social-group-mature-body = Allows media flagged as sensitive in this group. Everyone in the group must be 18 or older. Each member still sees it according to their own sensitive content settings.
social-group-invites = Group invites
social-group-my-nickname = My nickname
social-group-nickname = Nickname
social-group-make-owner = Make owner
social-group-remove = Remove
social-group-no-activity = No activity yet
social-group-members = Members — { $count }
social-group-owner = Owner
social-save = Save
social-close = Close
social-group-dm-captcha = The check this instance asks for could not be completed. Try again in a moment.
social-still-working = Still working on it…
social-group-dm-rate-limited = You have created or changed too many groups for now. Try again in { $wait }.
social-group-dm-limit = Too many group changes for now. Try again in { $wait }.
social-group-dm-not-addable = These people cannot be added: { $names }.
social-group-dm-failed = The group could not be created.
social-icon-too-large = The picture is larger than 10 MB.
social-icon-not-an-image = That file is not a PNG, JPEG, GIF or WebP picture.
social-icon-animated = Animated icons are not supported. Use a static image.

## Creating a group: groups that exist already, and people who cannot be added

social-duplicate-body = You already have a group with these users. Do you really want to create a new one? That's fine too!
social-duplicate-create = Create new group
social-unaddable-body = The following people can't be added to this group DM:
social-unaddable-rest = Create the group DM with the remaining { $count } { $count ->
    [one] recipient
   *[other] recipients
} and skip the others?
social-unaddable-none-left = No remaining recipients to create a group DM with.
social-unaddable-create = Create without them
social-unaddable-unknown = User not found
social-unaddable-blocked = You can't message this user
social-unaddable-not-friends = Not on your friends list
social-unaddable-disabled = Doesn't allow being added to group DMs
social-unaddable-other = Can't be added

## Nicknames

social-nickname-title = Change nickname
social-nickname-for = For { $name }
social-nickname = Nickname
social-nickname-clear = Clear
social-nickname-updated = Nickname updated
social-nickname-invalid = A nickname has at most 32 characters and no control characters.
social-friend-nickname-item = Change Friend Nickname
social-group-nickname-item = Change Group Nickname
social-group-my-nickname-item = Change My Group Nickname

## Owning a group

social-group-make-owner-item = Make Group Owner
social-group-make-owner-title = Transfer ownership
social-group-make-owner-body = Make { $name } the group owner? You'll lose owner privileges.
social-ownership-transferred = Ownership transferred
social-group-remove-item = Remove from Group
social-group-remove-title = Remove from group
social-group-remove-body = Remove { $name } from the group?
social-removed-from-group = Removed from group

## A group's invites

social-invites-owner-only = Only the group owner can manage invites.
social-invites-loading = Loading invites…
social-invites-failed = Failed to load invites. Try again.
social-invites-retry = Try again
social-invites-none = No invites created
social-invites-show-created = Show creation date instead of expiration date
social-invites-inviter = Inviter
social-invites-code = Code
social-invites-uses = Uses
social-invites-created = Created
social-invites-expires = Expires
social-invite-never = Never
social-invite-unknown = Unknown
social-invite-expired = Expired
social-invite-left-days = { $days }d { $hours }h left
social-invite-left-hours = { $hours }h { $minutes }m left
social-invite-left-minutes = { $minutes }m left
social-invite-copy = Copy
social-invite-copied = Invite link copied
social-invite-copied-label = Copied
social-invite-revoke = Revoke
social-invite-revoke-title = Revoke invite
social-invite-revoke-body = Revoke this invite? Can't be undone.
social-invite-revoked = Invite revoked
social-invite-for-friend = or send an invite to a friend:
social-invite-generate = Generate invite link
social-invite-create = Create
social-invite-expires-day = Your invite expires in 24 hours
social-invite-link-failed = Couldn't generate an invite link. Try again.

## Inviting someone to a community

social-community-item = Invite to Community
social-community-title = Invite to Community
social-community-body = Pick a community to send { $name } an invite to.
social-community-none = No community to invite to.
social-invite-sent = Invite sent for { $guild }

## Messages from blocked people

social-blocked-messages = { $count } blocked { $count ->
    [one] message
   *[other] messages
}
social-blocked-messages-hide = { $count } blocked { $count ->
    [one] message
   *[other] messages
} — hide

## Failures and waits

social-busy = That is already on its way.
social-action-failed = That did not work. Try again.
social-too-fast = Slow down a little, then try again.
social-not-connected = Not connected.
social-wait-seconds = { $count } { $count ->
    [one] second
   *[other] seconds
}
social-wait-minutes = { $count } { $count ->
    [one] minute
   *[other] minutes
}
social-wait-hours = { $count } { $count ->
    [one] hour
   *[other] hours
}
