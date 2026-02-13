#!/bin/bash
set -e

echo "🚀 Deploying landing pages to Cloudflare Pages..."
echo ""

# Check if user is logged in to wrangler
if ! wrangler whoami &>/dev/null; then
    echo "❌ Not logged in to Cloudflare. Please run:"
    echo "   wrangler login"
    exit 1
fi

echo "✓ Logged in to Cloudflare"
echo ""

# Deploy r8r
echo "📦 Deploying r8r..."
cd r8r
if ! wrangler pages project list 2>/dev/null | grep -q "r8r"; then
    echo "  Creating new project 'r8r'..."
    wrangler pages project create r8r --production-branch=main || true
fi
wrangler pages deploy . --project-name=r8r --branch=main
cd ..
echo ""

# Deploy zeptoclaw
echo "📦 Deploying zeptoclaw..."
cd zeptoclaw
if ! wrangler pages project list 2>/dev/null | grep -q "zeptoclaw"; then
    echo "  Creating new project 'zeptoclaw'..."
    wrangler pages project create zeptoclaw --production-branch=main || true
fi
wrangler pages deploy . --project-name=zeptoclaw --branch=main
cd ..
echo ""

echo "✅ Deployment complete!"
echo ""
echo "🌐 Your sites should be available at:"
echo "   https://r8r.pages.dev"
echo "   https://zeptoclaw.pages.dev"
echo ""
echo "💡 Tip: Add custom domains in the Cloudflare dashboard"
