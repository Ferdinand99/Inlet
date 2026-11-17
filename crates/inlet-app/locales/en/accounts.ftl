# Sign-in, the session screens and the session's connection.

## Start-up and offline

starting = Starting…
starting-connecting-to = Connecting to { $instance }…
offline-title = Offline: { $instance }
offline-retry-in = You are still signed in. Inlet tries again in { $seconds } s.
offline-retry-now = Retry now
about-link = About Inlet and licences

## Sign-in

sign-in-instance-label = Instance
sign-in-instance-placeholder = fluxer.opland.net
sign-in-token-label = Token
sign-in-token-placeholder = Paste your token
sign-in-mode-browser = Sign in with browser
sign-in-mode-token = Paste token
sign-in-browser-hint = Inlet opens the instance's sign-in page in your browser. Sign in there (passkeys, SSO and email codes all work) and enter the code Inlet shows.
sign-in-token-hint = The token is kept in your system keychain, never in a file. Inlet clears it from the clipboard after sign-in.
sign-in-open-page = Open the sign-in page
sign-in-submit = Sign in
signing-in = Signing in…
sign-in-enter-instance = Enter the address of a Fluxer instance.
sign-in-paste-first = Paste a token first.
sign-in-handoff-instructions = Your browser should now show the sign-in page. Sign in there, then enter this code:
sign-in-waiting = Waiting for the browser…
sign-in-reopen = Open the page again
sign-in-handoff-not-https = The instance gave a sign-in page that is not https. Inlet did not open it.
sign-in-denied = The sign-in was denied in the browser.
sign-in-code-expired = The code expired. Try again.

## Confirming a new sign-in

confirm-signed-in-as = You signed in as:
confirm-stored-after-continue = Inlet stores the session in your system keychain only after you continue.
confirm-cancel-pasted = Cancel only forgets the pasted token here. The app it came from stays signed in.
confirm-cancel-issued = Cancel ends this new session on the server.
confirm-continue = Continue

## The backend thread stopped

fatal-title = The network part of Inlet stopped.
fatal-hint = Restart it to reconnect. Your session is kept.
fatal-restart = Restart backend

## Notices after sign-out

notice-pasted-forgotten = The pasted token was only forgotten here. The app it came from stays signed in.
notice-not-deleted = The token could not be removed from the system keychain.
notice-sign-out-not-revoked = Signed out here, but the instance could not be told. End the session under Devices in Fluxer's settings.
notice-reject-not-revoked = The instance could not be told to end that session. End it under Devices in Fluxer's settings.

## Connection status (the line at the right of the channel header)

status-connecting = Connecting…
status-connecting-attempt = Connecting (attempt { $attempt })…
status-connected = Connected
status-disconnected = Disconnected
status-reconnecting = Reconnecting…
status-reconnecting-in = Disconnected, reconnecting in { $seconds } s…
status-gateway-reconnecting-in = Reconnecting in { $seconds } s…

## Why the gateway stopped

gateway-auth-failed = The instance ended this session. Sign in again.
gateway-api-version = The instance uses a newer gateway version than this Inlet build supports.
gateway-invalid-shard = The instance refused the gateway connection (invalid shard).
gateway-invalid-url = The instance's gateway address is not usable: { $url }
gateway-stopped = The gateway stopped: { $reason }

## Errors

error-timeout = The instance did not answer in time.
error-unauthorized = The instance did not accept the session. Sign in again.
error-global-rate-limit = Fluxer's global rate limit was hit, which ends the session. Sign in again.
error-api-refused = The instance refused: { $error }
error-unreachable = Could not reach the instance. Check the address and your connection.
error-endpoints-changed = This server's addresses changed. Sign in again.
error-bad-address = The instance address is not usable: { $reason }
error-too-large = The instance sent more data than Inlet accepts.
error-keychain-refused = Signed in, but the system keychain refused the token. You will have to sign in again next time.
error-task-panicked = Part of Inlet stopped unexpectedly ({ $what }). Try again.

## Several accounts (the account switcher)

accounts-title = Accounts
accounts-add = Add an account
accounts-full = At most { $max } accounts can be signed in.
accounts-active = Active
accounts-close = Close
accounts-handle-at = { $handle } on { $instance }
accounts-ended = { $account }: { $message }
sign-in-back = Back
sign-in-too-many-accounts = At most { $max } accounts can be signed in at once. Sign one out first.
error-account-id = The instance sent an account ID that Inlet cannot use.
error-account-mismatch = A stored session belongs to another account than Inlet expected, so it was not used. Sign in again.
notice-keychain-unreadable = The system keychain could not be read, so Inlet only forgot the account here and did not end its session on the server. End it under Devices in Fluxer's settings.
notice-account-list-unreadable = The list of accounts could not be read. It was kept as accounts.json.bad and is not written over; sign in again to use Inlet now.
notice-old-session-dropped = A session saved by an older Inlet could not be used safely and was removed. Sign in again, and end the old session under Devices in Fluxer's settings.
notice-account-list-moved = The list of accounts could not be read. It was moved to accounts.json.bad, where the accounts it names can still be found; sign in again to add them.

## Signing in inside the app

sign-in-mode-password = Sign in
sign-in-continue = Continue
sign-in-password-hint = Sign in with your account on the instance. Inlet talks to the instance directly; nothing goes through a browser.
sign-in-invite-note = Sign in to { $host } to open the invite.
link-prompt-invite-sign-in = Sign in on { $host }
entry-on-instance = On { $host }
entry-change-instance = Change instance
entry-use-browser = Sign in with the browser instead
entry-still-working = Still working on it…
entry-sso-enforced = This instance asks everyone to sign in with single sign-on, which Inlet cannot do yet. Use the browser sign-in.
entry-title-sign-in = Sign in
entry-title-welcome = Welcome back
entry-title-password = Enter your password
entry-title-code = Check your email
entry-title-mfa = Two-factor authentication
entry-title-ip = Check your email
entry-title-ip-lost = Connection lost
entry-label-email = Email
entry-label-username = Username
entry-label-password = Password
entry-label-code = Verification code
entry-code-placeholder = 6-digit code
entry-label-authenticator = Authenticator code
entry-label-backup = Backup code
entry-code-sent-to = Enter the code we sent to { $email }.
entry-mfa-text = Enter the 6-digit code from your authenticator app, or one of your backup codes.
entry-mfa-backup-only = Enter one of your backup codes.
entry-mfa-passkey-only = This account needs a passkey or security key, which Inlet cannot use yet. Sign in with the browser instead.
entry-send-new-code = Send a new code
entry-send-new-code-in = Send a new code in { $seconds } s
entry-different-email = Use a different email
entry-different-username = Use a different username
entry-back-to-sign-in = Back to sign in
entry-ip-text = Check your inbox at { $email } for the authorization link.
entry-ip-text-no-address = Check your inbox for the authorization link.
entry-ip-lost-text = Lost connection while waiting for authorization. Try again.
entry-ip-resend = Resend email
entry-ip-resend-in = Resend email ({ $seconds } s)
entry-ip-resent = Resent
entry-try-again = Try again
entry-no-account = No account was found for this. Creating an account in Inlet is not possible yet.
entry-no-account-closed = No account was found for this, and this instance is not taking new accounts right now.

## Why a sign-in step failed

entry-error-credentials-email = The email address or the password is wrong.
entry-error-credentials-username = The username or the password is wrong.
entry-error-code-wrong = That code is not right.
entry-error-code-expired = That code has expired. Ask for a new one.
entry-error-timed-out = This sign-in timed out. Start again.
entry-error-no-codes = This instance cannot send sign-in codes.
entry-error-sso-required = This instance asks everyone to sign in with single sign-on, which Inlet cannot do yet. Use the browser sign-in.
entry-error-feature-off = This instance does not use this way of signing in.
entry-error-network-blocked = The instance blocks this network.
entry-error-ip-resent-already = The email was already sent again. Check your inbox.
entry-error-resend-soon = Wait a moment before asking again.
entry-error-captcha = The instance asked for a check that Inlet could not pass. Try again, or sign in with the browser instead.
entry-error-rate-limited-now = Too many attempts. Wait a moment and try again.
entry-error-rate-limited-seconds = Too many attempts. Try again in { $seconds } s.
entry-error-rate-limited-minutes = Too many attempts. Try again in { $minutes } min.
entry-error-unknown-step = The instance asked for a step Inlet does not know yet. Sign in with the browser instead.
