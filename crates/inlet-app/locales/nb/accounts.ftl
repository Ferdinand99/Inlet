# Pålogging, øktvisningene og øktens tilkobling.

## Oppstart og frakoblet

starting = Starter…
starting-connecting-to = Kobler til { $instance }…
offline-title = Frakoblet: { $instance }
offline-retry-in = Du er fortsatt pålogget. Inlet prøver igjen om { $seconds } s.
offline-retry-now = Prøv igjen nå
about-link = Om Inlet og lisenser

## Pålogging

sign-in-instance-label = Instans
sign-in-instance-placeholder = fluxer.opland.net
sign-in-token-label = Token
sign-in-token-placeholder = Lim inn tokenet ditt
sign-in-mode-browser = Logg på med nettleseren
sign-in-mode-token = Lim inn token
sign-in-browser-hint = Inlet åpner påloggingssiden til instansen i nettleseren din. Logg på der (passnøkler, SSO og e-postkoder fungerer alle) og skriv inn koden Inlet viser.
sign-in-token-hint = Tokenet lagres i systemets nøkkelring, aldri i en fil. Inlet tømmer det fra utklippstavlen etter pålogging.
sign-in-open-page = Åpne påloggingssiden
sign-in-submit = Logg på
signing-in = Logger på…
sign-in-enter-instance = Skriv inn adressen til en Fluxer-instans.
sign-in-paste-first = Lim inn et token først.
sign-in-handoff-instructions = Nettleseren din skal nå vise påloggingssiden. Logg på der, og skriv så inn denne koden:
sign-in-waiting = Venter på nettleseren…
sign-in-reopen = Åpne siden igjen
sign-in-handoff-not-https = Instansen oppga en påloggingsside som ikke bruker https. Inlet åpnet den ikke.
sign-in-denied = Påloggingen ble avslått i nettleseren.
sign-in-code-expired = Koden er utløpt. Prøv igjen.

## Bekrefte en ny pålogging

confirm-signed-in-as = Du logget på som:
confirm-stored-after-continue = Inlet lagrer økten i systemets nøkkelring først når du fortsetter.
confirm-cancel-pasted = Avbryt glemmer bare det innlimte tokenet her. Appen det kom fra forblir pålogget.
confirm-cancel-issued = Avbryt avslutter denne nye økten på serveren.
confirm-continue = Fortsett

## Nettverksdelen stoppet

fatal-title = Nettverksdelen av Inlet stoppet.
fatal-hint = Start den på nytt for å koble til igjen. Økten din beholdes.
fatal-restart = Start nettverksdelen på nytt

## Meldinger etter utlogging

notice-pasted-forgotten = Det innlimte tokenet ble bare glemt her. Appen det kom fra forblir pålogget.
notice-not-deleted = Tokenet kunne ikke fjernes fra systemets nøkkelring.
notice-sign-out-not-revoked = Logget ut her, men instansen kunne ikke varsles. Avslutt økten under Enheter i Fluxers innstillinger.
notice-reject-not-revoked = Instansen kunne ikke varsles om å avslutte den økten. Avslutt den under Enheter i Fluxers innstillinger.

## Tilkoblingsstatus (linjen til høyre i kanaloverskriften)

status-connecting = Kobler til…
status-connecting-attempt = Kobler til (forsøk { $attempt })…
status-connected = Tilkoblet
status-disconnected = Frakoblet
status-reconnecting = Kobler til på nytt…
status-reconnecting-in = Frakoblet, kobler til på nytt om { $seconds } s…
status-gateway-reconnecting-in = Kobler til på nytt om { $seconds } s…

## Hvorfor gatewayen stoppet

gateway-auth-failed = Instansen avsluttet denne økten. Logg på igjen.
gateway-api-version = Instansen bruker en nyere gatewayversjon enn denne Inlet-versjonen støtter.
gateway-invalid-shard = Instansen avviste gateway-tilkoblingen (ugyldig shard).
gateway-invalid-url = Instansens gateway-adresse kan ikke brukes: { $url }
gateway-stopped = Gatewayen stoppet: { $reason }

## Feil

error-timeout = Instansen svarte ikke i tide.
error-unauthorized = Instansen godtok ikke økten. Logg på igjen.
error-global-rate-limit = Fluxers globale grense for forespørsler ble nådd, og det avslutter økten. Logg på igjen.
error-api-refused = Instansen avviste forespørselen: { $error }
error-unreachable = Kunne ikke nå instansen. Sjekk adressen og tilkoblingen din.
error-endpoints-changed = Adressene til denne serveren er endret. Logg på igjen.
error-bad-address = Instansadressen kan ikke brukes: { $reason }
error-too-large = Instansen sendte mer data enn Inlet godtar.
error-keychain-refused = Logget på, men systemets nøkkelring avviste tokenet. Du må logge på igjen neste gang.
error-task-panicked = En del av Inlet stoppet uventet ({ $what }). Prøv igjen.

## Flere kontoer (kontobyttet)

accounts-title = Kontoer
accounts-add = Legg til en konto
accounts-full = Høyst { $max } kontoer kan være pålogget.
accounts-active = Aktiv
accounts-close = Lukk
accounts-handle-at = { $handle } på { $instance }
accounts-ended = { $account }: { $message }
sign-in-back = Tilbake
sign-in-too-many-accounts = Høyst { $max } kontoer kan være pålogget samtidig. Logg ut av én først.
error-account-id = Instansen sendte en konto-ID som Inlet ikke kan bruke.
error-account-mismatch = En lagret økt tilhører en annen konto enn Inlet forventet, så den ble ikke brukt. Logg på igjen.
notice-keychain-unreadable = Systemets nøkkelring kunne ikke leses, så Inlet glemte bare kontoen her og avsluttet ikke økten på serveren. Avslutt den under Enheter i Fluxers innstillinger.
notice-account-list-unreadable = Kontolisten kunne ikke leses. Den ble beholdt som accounts.json.bad og skrives ikke over. Logg på igjen for å bruke Inlet nå.
notice-old-session-dropped = En økt lagret av en eldre Inlet-versjon kunne ikke brukes trygt og ble fjernet. Logg på igjen, og avslutt den gamle økten under Enheter i Fluxers innstillinger.
notice-account-list-moved = Kontolisten kunne ikke leses. Den ble flyttet til accounts.json.bad, der kontoene den nevner fortsatt kan finnes. Logg på igjen for å legge dem til.
