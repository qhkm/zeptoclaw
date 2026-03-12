# Webex Channel Configuration Example
# Add this to your ~/.zeptoclaw/config.json

## Configuration Structure

```json
{
  "channels": {
    "webex": {
      "enabled": true,
      "access_token": "YOUR_WEBEX_BOT_ACCESS_TOKEN",
      "webhook_url": "https://yourdomain.com/webhook",
      "webhook_secret": "optional-but-recommended-secret",
      "bind_address": "0.0.0.0",
      "port": 8084,
      "allow_from": [],
      "deny_by_default": false
    }
  }
}
```

## How to Get Your Webex Bot Token

1. Go to https://developer.webex.com/
2. Sign in with your Webex account
3. Navigate to "My Webex Apps" → "Create a New App"
4. Select "Create a Bot"
5. Fill in:
   - Bot name
   - Bot username
   - Icon (optional)
   - Description
6. Click "Add Bot"
7. Copy the **Bot Access Token** (this is your `access_token`)

## Webhook Setup

### Option 1: Public Webhook (Recommended for Production)

1. You need a publicly accessible URL (e.g., `https://yourdomain.com/webhook`)
2. Set `webhook_url` to your public URL
3. Set a `webhook_secret` for signature verification
4. Configure `bind_address` and `port` for the local server

### Option 2: Development with ngrok

```bash
# Terminal 1: Start zeptoclaw gateway
zeptoclaw gateway --config ~/.zeptoclaw/config.json

# Terminal 2: Expose with ngrok
ngrok http 8084
```

Then update your config:
```json
{
  "channels": {
    "webex": {
      "webhook_url": "https://YOUR-NGROK-ID.ngrok.io/webhook",
      "port": 8084
    }
  }
}
```

## Security Settings

### Allow Specific Users Only

```json
{
  "channels": {
    "webex": {
      "allow_from": [
        "user1@company.com-person-id",
        "user2@company.com-person-id"
      ],
      "deny_by_default": true
    }
  }
}
```

To find a person ID:
- Go to https://developer.webex.com/docs/api/v1/people/list-people
- Use the API to search by email

### Webhook Signature Verification

Always set a `webhook_secret` in production:

```json
{
  "channels": {
    "webex": {
      "webhook_secret": "your-random-secret-string-here"
    }
  }
}
```

## Testing Your Bot

1. Add your bot to a Webex space
2. Send a message: `@YourBot hello`
3. Check zeptoclaw logs for incoming webhooks
4. The bot should respond according to your agent configuration

## Troubleshooting

### Bot doesn't receive messages
- Check that `webhook_url` is publicly accessible
- Verify the URL in Webex Developer portal (Webhooks section)
- Check zeptoclaw logs for webhook errors
- Ensure port is not blocked by firewall

### Invalid signature errors
- Verify `webhook_secret` matches what you configured in Webex
- Check if the secret has special characters (URL-encode if needed)

### Bot responds to its own messages
- This is prevented automatically by checking bot ID
- If you see loops, check the logs

## Features Supported

✅ **Sending Messages** - Text responses via REST API
✅ **Receiving Messages** - Webhooks for inbound messages  
✅ **File Attachments** - Download and process files
✅ **Room Detection** - Works in 1:1 and group spaces
✅ **Allowlist** - Restrict access by person ID
✅ **Signature Verification** - HMAC-SHA1 webhook security

## Example Full Configuration

```json
{
  "agents": {
    "defaults": {
      "model": "gpt-4",
      "max_tokens": 16384
    }
  },
  "channels": {
    "webex": {
      "enabled": true,
      "access_token": "YzJh...your-long-token...M2Y",
      "webhook_url": "https://mybot.example.com/webhook",
      "webhook_secret": "super-secret-signature-key",
      "bind_address": "0.0.0.0",
      "port": 8084,
      "allow_from": [],
      "deny_by_default": false
    }
  },
  "providers": {
    "openai": {
      "api_key": "sk-..."
    }
  }
}
```

## Next Steps

1. Create your bot at https://developer.webex.com/
2. Get the access token
3. Update your config.json
4. Start zeptoclaw: `zeptoclaw gateway`
5. Add bot to a Webex space and test!

For more information, see:
- Webex Bot Documentation: https://developer.webex.com/docs/bots
- Webex Webhooks Guide: https://developer.webex.com/docs/api/guides/webhooks
