import os
import sys
import yaml
import subprocess
import asyncio
from telegram import Update, InlineKeyboardButton, InlineKeyboardMarkup
from telegram.ext import Application, CommandHandler, CallbackQueryHandler, ContextTypes

BASE_DIR = "/home/vth/Vth/ml_engine"
CONFIG_PATH = os.path.join(BASE_DIR, "config.yaml")

def load_config():
    with open(CONFIG_PATH, "r") as f:
        return yaml.safe_load(f)

def get_telegram_creds():
    config = load_config()
    tg_cfg = config.get("telegram", {})
    return tg_cfg.get("bot_token"), str(tg_cfg.get("chat_id"))

BOT_TOKEN, ADMIN_CHAT_ID = get_telegram_creds()

async def approve(update: Update, context: ContextTypes.DEFAULT_TYPE):
    # Check permissions
    if str(update.effective_user.id) != ADMIN_CHAT_ID:
        await update.message.reply_text("⛔ 未授权操作。")
        return

    job_id = " ".join(context.args) if context.args else None
    if not job_id:
        await update.message.reply_text("❓ 请提供任务ID。例如: /approve_retrain 20260727_115932")
        return

    pkg_path = os.path.join(BASE_DIR, "data", "retrain_packages", job_id)
    if not os.path.exists(pkg_path):
        await update.message.reply_text(f"❓ 任务包不存在: {job_id}")
        return

    keyboard = [
        [
            InlineKeyboardButton("✅ 批准重训", callback_data=f"approve_{job_id}"),
            InlineKeyboardButton("❌ 拒绝", callback_data="reject"),
        ]
    ]
    reply_markup = InlineKeyboardMarkup(keyboard)
    
    await update.message.reply_text(
        f"📋 收到重训审批请求 (任务: {job_id})\n\n请确认是否批准？",
        reply_markup=reply_markup
    )

async def button_callback(update: Update, context: ContextTypes.DEFAULT_TYPE):
    query = update.callback_query
    await query.answer()

    if str(update.effective_user.id) != ADMIN_CHAT_ID:
        await query.edit_message_text("⛔ 未授权操作。")
        return

    data = query.data
    if data.startswith("approve_"):
        job_id = data.replace("approve_", "")
        await query.edit_message_text(f"⏳ 正在执行重训任务: {job_id}...\n\n这可能需要几分钟，请耐心等待。完成后会通知您。")
        
        # Async execution of the bash script
        script_path = os.path.join(BASE_DIR, "data", "retrain_packages", job_id, "run_train.sh")
        
        if not os.path.exists(script_path):
            await query.edit_message_text(f"❌ 任务脚本不存在: {script_path}")
            return
            
        command = f"cd {BASE_DIR} && bash {script_path}"
        
        # Run in background
        process = await asyncio.create_subprocess_shell(
            command,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE
        )
        
        stdout, stderr = await process.communicate()
        
        if process.returncode == 0:
            # We also restart the hyperq-ml.service automatically for convenience
            restart_cmd = "sudo systemctl restart hyperq-ml.service"
            restart_process = await asyncio.create_subprocess_shell(
                restart_cmd,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE
            )
            await restart_process.communicate()
            
            output_tail = stdout.decode('utf-8')[-1000:]
            msg = f"✅ 重训任务 `{job_id}` 成功完成！模型服务已自动重启。\n\n```\n{output_tail}\n```"
            if len(msg) > 4000:
                msg = msg[-4000:]
            await query.edit_message_text(msg, parse_mode="Markdown")
        else:
            error_tail = stderr.decode('utf-8')[-1000:]
            msg = f"❌ 重训任务 `{job_id}` 失败！\n\n错误: ```\n{error_tail}\n```"
            if len(msg) > 4000:
                msg = msg[-4000:]
            await query.edit_message_text(msg, parse_mode="Markdown")
            
    elif data == "reject":
        await query.edit_message_text("❌ 已拒绝本次重训请求。")

def main():
    if not BOT_TOKEN or BOT_TOKEN == 'YOUR_BOT_TOKEN_HERE':
        print("Bot token not configured. Exiting.")
        sys.exit(1)
        
    print(f"Starting Telegram Approval Bot... (Admin Chat ID: {ADMIN_CHAT_ID})")
    app = Application.builder().token(BOT_TOKEN).build()
    app.add_handler(CommandHandler("approve_retrain", approve))
    app.add_handler(CallbackQueryHandler(button_callback))
    
    app.run_polling()

if __name__ == "__main__":
    main()
