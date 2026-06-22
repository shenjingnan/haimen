#!/usr/bin/env python3
"""
小智硬件设备 Echo 回环测试脚本

模拟硬件设备连接 haimen xiaozhi WebSocket 端点：
1. OTA 握手 → 获取 WebSocket URL
2. WebSocket 连接 → HELLO 握手
3. 发送音频帧（BinaryProtocol2 格式）
4. 发送 listen.stop → 验证回声回放

用法:
    python scripts/test_xiaozhi_echo.py [--host localhost] [--port 9527]
"""

import argparse
import asyncio
import json
import struct
import sys
import time

try:
    import websockets
except ImportError:
    print("请先安装 websockets: pip install websockets")
    sys.exit(1)


def make_protocol2_frame(payload: bytes, timestamp: int = 0) -> bytes:
    """编码 BinaryProtocol2 帧"""
    frame = bytearray(16)
    struct.pack_into(">H", frame, 0, 2)       # Version = 2
    struct.pack_into(">H", frame, 2, 0)        # Type = 0 (opus)
    struct.pack_into(">I", frame, 4, 0)        # Reserved = 0
    struct.pack_into(">I", frame, 8, timestamp) # Timestamp
    struct.pack_into(">I", frame, 12, len(payload))  # Payload Size
    frame.extend(payload)
    return bytes(frame)


def make_protocol3_frame(payload: bytes) -> bytes:
    """编码 BinaryProtocol3 帧"""
    frame = bytearray(4)
    frame[0] = 0        # Type = 0 (opus)
    frame[1] = 0        # Reserved = 0
    struct.pack_into(">H", frame, 2, len(payload))  # Payload Size
    frame.extend(payload)
    return bytes(frame)


async def test_ota(host: str, port: int):
    """测试 OTA 端点"""
    import httpx
    url = f"http://{host}:{port}/xiaozhi/ota/"
    headers = {"Device-Id": "AA:BB:CC:DD:EE:FF"}
    body = {
        "version": 2,
        "mac_address": "AA:BB:CC:DD:EE:FF",
        "uuid": "test-uuid",
    }
    async with httpx.AsyncClient() as client:
        resp = await client.post(url, json=body, headers=headers)
        data = resp.json()
        print(f"✅ OTA 响应: {json.dumps(data, indent=2)}")
        assert "websocket" in data
        assert "audio_params" in data
        return data.get("websocket", {}).get("url", f"ws://{host}:{port}/xiaozhi/ws")


async def test_websocket_hello(ws_url: str):
    """测试 HELLO 握手"""
    async with websockets.connect(
        ws_url,
        additional_headers={"Device-Id": "AA:BB:CC:DD:EE:FF"}
    ) as ws:
        # 发送 HELLO
        hello = {
            "type": "hello",
            "version": 2,
            "transport": "websocket",
            "audio_params": {
                "format": "opus",
                "sample_rate": 24000,
                "channels": 1,
                "frame_duration": 60,
            },
            "features": {},
        }
        await ws.send(json.dumps(hello))
        resp = await ws.recv()
        data = json.loads(resp)
        print(f"✅ HELLO 响应: session_id={data.get('session_id', 'MISSING')}")
        assert data["type"] == "hello"
        assert "session_id" in data
        return data["session_id"]


async def test_echo_playback(ws_url: str):
    """测试完整 Echo 回环"""
    async with websockets.connect(
        ws_url,
        additional_headers={"Device-Id": "BB:CC:DD:EE:FF:00"}
    ) as ws:
        # 1. HELLO 握手
        await ws.send(json.dumps({
            "type": "hello",
            "version": 2,
            "transport": "websocket",
            "audio_params": {
                "format": "opus",
                "sample_rate": 24000,
                "channels": 1,
                "frame_duration": 60,
            },
            "features": {},
        }))
        await ws.recv()
        print("✅ 1. HELLO 握手完成")

        # 2. 发送 listen.start
        await ws.send(json.dumps({"type": "listen", "state": "start", "mode": "auto"}))
        await asyncio.sleep(0.1)
        print("✅ 2. 发送 listen.start")

        # 3. 发送音频帧（模拟 Opus 数据）
        dummy_opus = b"\x80\x00\x00\x00"  # 有效的 Opus TOC 帧
        frames = [
            make_protocol2_frame(dummy_opus, timestamp=i * 60)
            for i in range(3)
        ]
        for i, frame in enumerate(frames):
            await ws.send(frame)
            print(f"✅ 3.{i+1} 发送音频帧 {i+1}")
            await asyncio.sleep(0.05)

        # 4. 发送 listen.stop → 触发回声回放
        await ws.send(json.dumps({"type": "listen", "state": "stop"}))
        print("✅ 4. 发送 listen.stop，等待回放...")

        # 5. 接收回放数据
        timeout = 5.0
        start = time.time()
        received_binary = False
        received_tts_stop = False

        while time.time() - start < timeout:
            try:
                msg = await asyncio.wait_for(ws.recv(), timeout=1.0)
                if isinstance(msg, str):
                    data = json.loads(msg)
                    print(f"  ← 收到文本: type={data.get('type')}, state={data.get('state')}")
                    if data.get("type") == "tts":
                        if data.get("state") == "start":
                            print("  ✅ TTS.start 收到")
                        elif data.get("state") == "stop":
                            print("  ✅ TTS.stop 收到")
                            received_tts_stop = True
                            break
                else:
                    received_binary = True
                    print(f"  ← 收到二进制帧: {len(msg)} 字节")
            except asyncio.TimeoutError:
                break

        # 验证
        assert received_tts_stop, "❌ 未收到 TTS.stop"
        assert received_binary, "❌ 未收到二进制音频帧"
        print("\n✅✅✅ Echo 回环测试通过！")


async def main():
    parser = argparse.ArgumentParser(description="测试小智硬件设备 Echo 回环")
    parser.add_argument("--host", default="localhost", help="haimen 服务器地址")
    parser.add_argument("--port", type=int, default=9527, help="haimen 服务器端口")
    parser.add_argument("--echo-only", action="store_true", help="只测试 Echo 回环")
    args = parser.parse_args()

    print(f"🔌 测试服务器: {args.host}:{args.port}\n")

    if not args.echo_only:
        # 测试 OTA
        ws_url = await test_ota(args.host, args.port)
        print()

        # 测试 HELLO
        await test_websocket_hello(ws_url)
        print()

    # 测试 Echo 回环
    ws_url = f"ws://{args.host}:{args.port}/xiaozhi/ws"
    await test_echo_playback(ws_url)


if __name__ == "__main__":
    asyncio.run(main())
