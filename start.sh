#!/bin/bash
cd /home/vth/Vth/hyperq-rs
source $HOME/.cargo/env

if pgrep -x hyperq-rs > /dev/null; then
    echo "HyperQ-rs is already running!"
    exit 1
fi

echo "Starting HyperQ V4.1 in the background..."
nohup ./target/release/hyperq-rs > hyperq.log 2>&1 &
echo "Started! PID: $!"
echo "Use 'tail -f hyperq.log' to view logs."
