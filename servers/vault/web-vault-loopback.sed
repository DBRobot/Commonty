# The web vault's three https checks (its api calls, its notifications, the
# anonymous hub) also pass the app's own two origins (nix/modules/vault/vaultwarden.nix)
s#if\(!e\.url\.startsWith\("https://"\)&&#if(!e.url.startsWith("https://")\&\&!/^http:\\/\\/(127\\.0\\.0\\.1|v-[0-9a-f]{32}\\.localhost):41662\\//.test(e.url)\&\&#
s#connect\$\(e,t\)\{if\(!t\.startsWith\("https://"\)&&#connect$(e,t){if(!t.startsWith("https://")\&\&!/^http:\\/\\/(127\\.0\\.0\\.1|v-[0-9a-f]{32}\\.localhost):41662\\//.test(t)\&\&#
s#!this\.url\.startsWith\("https://"\)&&#!this.url.startsWith("https://")\&\&!/^http:\\/\\/(127\\.0\\.0\\.1|v-[0-9a-f]{32}\\.localhost):41662\\//.test(this.url)\&\&#
