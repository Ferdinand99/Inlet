# Venner, venneforespørsler, blokkeringer, handlinger i DM-listen og gruppechatter.

## DM-listen

social-friends = Venner
social-personal-notes = Personlige notater
social-pin-dm = Fest direktemeldingssamtale
social-unpin-dm = Løsne direktemeldingssamtale
social-close-dm = Lukk direktemeldingssamtale
social-group-settings = Gruppeinnstillinger

## Vennesiden

social-tab-online = Pålogget
social-tab-all = Alle
social-tab-pending = Ventende
social-tab-blocked = Blokkert
social-add-friend = Legg til venn
social-search = Søk
social-online-count = Pålogget — { $count }
social-all-count = Alle venner — { $count }
social-pending-count = Ventende — { $count }
social-blocked-count = Blokkert — { $count }
social-empty-online = Ingen venner er pålogget akkurat nå.
social-empty-all = Ingen venner ennå. Legg til en med brukernavnet.
social-empty-pending = Det finnes ingen ventende venneforespørsler.
social-empty-blocked = Du har ikke blokkert noen.
social-incoming-request = Innkommende venneforespørsel
social-outgoing-request = Utgående venneforespørsel
social-blocked = Blokkert
social-message = Send melding
social-accept-request = Godta venneforespørsel
social-cancel-request = Trekk tilbake venneforespørsel
social-unblock = Opphev blokkering

## Legge til en venn

social-add-friend-body = Du kan legge til venner med brukernavnet deres.
social-add-friend-placeholder = Brukernavn, eller brukernavn#0000
social-add-friend-send = Send venneforespørsel
social-add-friend-sent = Venneforespørselen din til { $name } er sendt.
social-add-friend-invalid = Skriv inn et brukernavn på opptil 32 tegn, eventuelt med en #tagg på 1–4 sifre.
social-sending = Sender…

## Bekreftelser

social-remove-friend = Fjern venn
social-remove-friend-title = Fjerne { $name }?
social-remove-friend-body = Er du sikker på at du vil fjerne { $name } fra vennene dine?
social-block = Blokker
social-block-title = Blokkere { $name }?
social-block-body = { $name } blir fjernet fra vennene dine, og meldingene deres blir skjult.
social-leave-group = Forlat gruppe
social-leave-group-title = Forlate { $name }?
social-leave-group-body = Du kan ikke bli med i { $name } igjen med mindre du blir lagt til på nytt.

## Gruppechatter

social-new-dm-title = Velg venner
social-new-dm-body = Du kan legge til { $count } { $count ->
    [one] venn til.
   *[other] venner til.
}
social-new-dm-open = Åpne direktemeldingssamtale
social-new-group-create = Opprett gruppechat
social-group-dm-search = Skriv inn brukernavnet til en venn
social-group-no-friends = Ingen venner å legge til.
social-group-add-title = Legg til venner i { $name }
social-group-add-body = Velg en venn du vil legge til i gruppen.
social-group-add = Legg til venner
social-group-name = Gruppenavn
social-group-name-placeholder = Gruppenavn
social-group-icon-prompt = Velg et gruppeikon
social-group-icon-change = Endre ikon
social-group-icon-remove = Fjern ikon
social-group-members = Medlemmer — { $count }
social-group-owner = Eier
social-save = Lagre
social-close = Lukk
social-group-dm-captcha = Denne instansen krever en CAPTCHA som Inlet ikke kunne løse. Opprett gruppen i nettleseren din, eller prøv igjen senere.
social-group-dm-rate-limited = Du har opprettet eller endret for mange grupper foreløpig. Prøv igjen om { $wait }.
social-group-dm-limit = For mange gruppeendringer foreløpig. Prøv igjen om { $wait }.
social-group-dm-not-addable = Disse personene kan ikke legges til: { $names }.
social-group-dm-failed = Gruppen kunne ikke opprettes.
social-icon-too-large = Bildet er større enn 10 MB.
social-icon-not-an-image = Den filen er ikke et bilde i PNG, JPEG, GIF eller WebP.

## Meldinger fra blokkerte personer

social-blocked-messages = { $count } { $count ->
    [one] blokkert melding
   *[other] blokkerte meldinger
}
social-blocked-messages-hide = { $count } { $count ->
    [one] blokkert melding
   *[other] blokkerte meldinger
} — skjul

## Feil og ventetid

social-busy = Det er allerede på vei.
social-action-failed = Det fungerte ikke. Prøv igjen.
social-too-fast = Ta det litt rolig, og prøv igjen.
social-not-connected = Ikke tilkoblet.
social-wait-seconds = { $count } { $count ->
    [one] sekund
   *[other] sekunder
}
social-wait-minutes = { $count } { $count ->
    [one] minutt
   *[other] minutter
}
social-wait-hours = { $count } { $count ->
    [one] time
   *[other] timer
}
