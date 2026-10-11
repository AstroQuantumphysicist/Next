"""Loopback-only bounded HTTP peer for Next's original TCP client test."""
import socket
import threading

REQUEST = b"GET / HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n"
RESPONSE = b"HTTP/1.0 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nNEXTTCP1pong"


class Responder:
    def __init__(self):
        self.socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
        self.socket.bind(("127.0.0.1", 39098))
        self.socket.listen(8)
        self.socket.settimeout(0.1)
        self.stop = threading.Event()
        self.requests = 0
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()

    def run(self):
        while not self.stop.is_set() and self.requests < 1024:
            try:
                connection, peer = self.socket.accept()
                with connection:
                    connection.settimeout(0.5)
                    if peer[0] != "127.0.0.1":
                        continue
                    data = bytearray()
                    while b"\r\n\r\n" not in data and len(data) < 512:
                        part = connection.recv(512 - len(data))
                        if not part:
                            break
                        data.extend(part)
                    if data == REQUEST:
                        connection.sendall(RESPONSE)
                        self.requests += 1
            except socket.timeout:
                continue
            except OSError:
                if self.stop.is_set():
                    return

    def close(self):
        self.stop.set()
        self.socket.close()
        self.thread.join(timeout=1)
