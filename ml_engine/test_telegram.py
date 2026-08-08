import yaml
import requests

with open('/home/vth/Vth/ml_engine/config.yaml', 'r') as f:
    config = yaml.safe_load(f)

telegram_cfg = config.get('telegram', {})
bot_token = telegram_cfg.get('bot_token', '')
chat_id = telegram_cfg.get('chat_id', '')

if not bot_token or not chat_id:
    print("Token or chat_id is missing!")
    exit(1)

url = f"https://api.telegram.org/bot{bot_token}/sendMessage"
msg = "✅ *HyperQ Drift Monitor* \\n\\nThis is a test message. Your Telegram bot configuration is working perfectly! I will notify you here if the model requires retraining."

payload = {
    "chat_id": chat_id,
    "text": msg,
    "parse_mode": "Markdown"
}

try:
    r = requests.post(url, json=payload, timeout=10)
    r.raise_for_status()
    print("Test message sent successfully!")
except Exception as e:
    print(f"Failed to send test message: {e}")
