# Passkeys and WebAuthn

<!-- index: design | Passkey support status, Apple entitlement, and the signing path | when a sign-in offers a passkey -->

Relay hosts every page in WKWebView. WebKit's default WKWebView host is not a
browser: `navigator.credentials.create()` / `.get()` for a public-key
credential are refused unless the app carries Apple's restricted browser
entitlement. That is the whole reason passkeys do not work in an ordinary
build, and no JavaScript shim can grant it.

## Current status

- The Settings → Passwords row states **Not available in this build**.
- `scripts/Relay.entitlements` carries the entitlement and the associated
  domains shape. It is a template, not applied to the default ad-hoc build.
- The default `scripts/bundle-macos.sh` produces an ad-hoc-signed app, so
  Safari-style passkeys are not available there.
- Relay does not replace `navigator.credentials` in the page. A shim cannot
  create a platform credential and would only hide the real error.

## What the owner must do

1. Request `com.apple.developer.web-browser.public-key-credential` from Apple
   (the entitlement browsers such as Chrome, Firefox, and Search use).
2. Create a Developer ID provisioning profile that includes the entitlement.
3. Build the bundle with the profile and the template:

   ```bash
   RELAY_PASSKEYS=1 \
   RELAY_PROVISIONPROFILE="$PWD/Relay.provisionprofile" \
   bash scripts/bundle-macos.sh
   ```

4. Verify the signature carries the entitlement:

   ```bash
   codesign -d --entitlements :- dist/Relay.app
   ```

5. On a signed build, open <https://webauthn.io> and create a passkey. The
   system sheet should appear. If it does not, the entitlement or profile is
   missing; check step 4 before changing code.

## Known limits

- Conditional mediation (a passkey offered under the name field) is not
  implemented. A page that calls `navigator.credentials.get({ mediation:
  "conditional" })` will not get a Relay-provided list; use the explicit
  passkey button on the site.
- Creating and asserting a credential is WebKit's job once entitled. Relay
  deliberately does not re-implement the ceremony; Search does because it
  worked around a WebKit operation-leak bug, which is a much larger surface.
- Private tabs use a non-persistent data store, so a passkey ceremony there
  must be expected to fail closed.
