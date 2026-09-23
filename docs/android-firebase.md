# Android Firebase push

LAM uses Firebase Cloud Messaging only as a wake-up signal for critical items. The data message contains notification-safe metadata; the Android app refreshes the canonical item from the LAM Worker before enabling actions.

## Configuration

- `android/app/google-services.json` contains the public Android client configuration for both `dev.carraes.lam` and `dev.carraes.lam.debug`. It contains no service-account private key and is committed.
- `worker/wrangler.jsonc` contains the public `FCM_PROJECT_ID` variable.
- Cloudflare stores the service-account JSON only as the encrypted Worker secret `FCM_SERVICE_ACCOUNT_JSON`.
- The dedicated service account has a project custom role containing only `cloudmessaging.messages.create`.

The Firebase project is `lam-push-carraes` (project number `273773536447`). The sender account is
`lam-fcm-sender@lam-push-carraes.iam.gserviceaccount.com`; its custom role is `lamFcmSender`.

The existing D1 `devices.fcm_token` column stores the Firebase Installation ID (FID). The old column name remains for API and migration compatibility.

## Replace the Worker credential

Create a new key for the dedicated LAM FCM sender service account, then stream the downloaded JSON directly to Wrangler from outside the repository:

```sh
cd worker
npx wrangler secret put FCM_SERVICE_ACCOUNT_JSON < /absolute/path/to/new-key.json
```

Deploy the Worker and verify one live critical notification before deleting the previous key in Google Cloud IAM. Never commit the key or paste its contents into shell arguments, logs, or issue trackers.

## Remove push delivery

Delete the Cloudflare secret and redeploy:

```sh
cd worker
npx wrangler secret delete FCM_SERVICE_ACCOUNT_JSON
npx wrangler deploy
```

Then disable or delete the sender service account in Google Cloud IAM. The Android app continues to sync normally; only background critical notifications stop.

## Verification

After changing Firebase configuration:

```sh
cd android
./gradlew testDebugUnitTest lintDebug assembleDebug assembleRelease

cd ../worker
npx tsc --noEmit
npm test -- --run
npx wrangler deploy --dry-run
```

For a live check, background the Android app, create one critical request and one normal/FYI item, and confirm that only the critical request produces a system notification. Resolving that request from the TUI must remove its notification.

On 2026-09-22, a critical request reached the paired SM-S928B release app over FCM while LAM was backgrounded. Retracting it removed the notification. The debug app was also paired, so it received the same test push; two installed, paired app variants will produce two alerts.
