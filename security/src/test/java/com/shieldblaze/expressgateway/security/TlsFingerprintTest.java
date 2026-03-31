/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.security;

import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

class TlsFingerprintTest {

    @Test
    void nullInputReturnsNull() {
        assertNull(TlsFingerprint.fromClientHello(null));
    }

    @Test
    void tooSmallBufferReturnsNull() {
        ByteBuf buf = Unpooled.wrappedBuffer(new byte[10]);
        assertNull(TlsFingerprint.fromClientHello(buf));
        buf.release();
    }

    @Test
    void nonHandshakeContentTypeReturnsNull() {
        ByteBuf buf = Unpooled.buffer(50);
        buf.writeByte(23); // Application Data, not Handshake (22)
        buf.writeShort(0x0303);
        buf.writeShort(45);
        buf.writeZero(45);
        assertNull(TlsFingerprint.fromClientHello(buf));
        buf.release();
    }

    @Test
    void nonClientHelloHandshakeReturnsNull() {
        ByteBuf buf = Unpooled.buffer(100);
        buf.writeByte(22); // Handshake
        buf.writeShort(0x0303); // TLS 1.2
        buf.writeShort(50); // Record length
        buf.writeByte(2); // ServerHello, not ClientHello (1)
        buf.writeZero(49);
        assertNull(TlsFingerprint.fromClientHello(buf));
        buf.release();
    }

    @Test
    void validClientHelloExtraction() {
        // Build a minimal TLS 1.2 ClientHello
        ByteBuf payload = Unpooled.buffer(256);

        // Handshake type: ClientHello
        payload.writeByte(1);
        // Handshake length placeholder (3 bytes)
        int handshakeLenIdx = payload.writerIndex();
        payload.writeZero(3);
        int handshakeStart = payload.writerIndex();

        // Client Version: TLS 1.2 (0x0303)
        payload.writeShort(0x0303);

        // Random (32 bytes)
        payload.writeZero(32);

        // Session ID length: 0
        payload.writeByte(0);

        // Cipher Suites: 2 suites
        payload.writeShort(4); // 2 suites * 2 bytes
        payload.writeShort(0x1301); // TLS_AES_128_GCM_SHA256
        payload.writeShort(0x1302); // TLS_AES_256_GCM_SHA384

        // Compression methods: 1 method (null)
        payload.writeByte(1);
        payload.writeByte(0);

        // Extensions: SNI (type 0)
        int extLenIdx = payload.writerIndex();
        payload.writeShort(0); // Extensions length placeholder
        int extStart = payload.writerIndex();

        // Extension: supported_groups (type 10)
        payload.writeShort(10); // type
        payload.writeShort(4);  // extension data length
        payload.writeShort(2);  // groups list length
        payload.writeShort(0x0017); // secp256r1

        // Extension: ec_point_formats (type 11)
        payload.writeShort(11); // type
        payload.writeShort(2);  // extension data length
        payload.writeByte(1);   // formats list length
        payload.writeByte(0);   // uncompressed

        // Fix extensions length
        int extLen = payload.writerIndex() - extStart;
        payload.setShort(extLenIdx, extLen);

        // Fix handshake length
        int handshakeLen = payload.writerIndex() - handshakeStart;
        payload.setMedium(handshakeLenIdx, handshakeLen);

        // Now wrap in TLS record
        ByteBuf record = Unpooled.buffer(5 + payload.readableBytes());
        record.writeByte(22); // Content type: Handshake
        record.writeShort(0x0301); // Record version: TLS 1.0 (common in record layer)
        record.writeShort(payload.readableBytes());
        record.writeBytes(payload);

        TlsFingerprint fp = TlsFingerprint.fromClientHello(record);
        assertNotNull(fp, "Should extract fingerprint from valid ClientHello");

        assertEquals(0x0303, fp.tlsVersion(), "TLS version should be 0x0303 (TLS 1.2)");
        assertEquals(2, fp.cipherSuites().size(), "Should have 2 cipher suites");
        assertTrue(fp.cipherSuites().contains(0x1301));
        assertTrue(fp.cipherSuites().contains(0x1302));
        assertEquals(2, fp.extensions().size(), "Should have 2 extensions");
        assertTrue(fp.extensions().contains(10)); // supported_groups
        assertTrue(fp.extensions().contains(11)); // ec_point_formats
        assertEquals(1, fp.ellipticCurves().size());
        assertEquals(0x0017, fp.ellipticCurves().getFirst());
        assertEquals(1, fp.ecPointFormats().size());
        assertEquals(0, fp.ecPointFormats().getFirst());

        assertNotNull(fp.ja3String());
        assertFalse(fp.ja3String().isEmpty());
        assertNotNull(fp.ja3Hash());
        assertEquals(32, fp.ja3Hash().length(), "MD5 hex should be 32 chars");

        // Verify reader index was not modified
        assertEquals(0, record.readerIndex(), "Reader index should be restored");

        payload.release();
        record.release();
    }

    @Test
    void blockListOperations() {
        TlsFingerprint.BlockList blockList = new TlsFingerprint.BlockList();

        assertEquals(0, blockList.size());

        blockList.add("abc123");
        assertTrue(blockList.isBlocked("abc123"));
        assertFalse(blockList.isBlocked("def456"));
        assertEquals(1, blockList.size());

        blockList.remove("abc123");
        assertFalse(blockList.isBlocked("abc123"));
        assertEquals(0, blockList.size());
    }

    @Test
    void blockListWithFingerprint() {
        TlsFingerprint.BlockList blockList = new TlsFingerprint.BlockList();

        // Build a fingerprint manually
        TlsFingerprint fp = new TlsFingerprint(
                0x0303, java.util.List.of(), java.util.List.of(),
                java.util.List.of(), java.util.List.of(),
                "771,,,", "somehash"
        );

        blockList.add("somehash");
        assertTrue(blockList.isBlocked(fp));

        blockList.clear();
        assertFalse(blockList.isBlocked(fp));
    }

    @Test
    void greaseValuesAreExcluded() {
        // Build a ClientHello with GREASE cipher suite
        ByteBuf payload = Unpooled.buffer(256);
        payload.writeByte(1); // ClientHello
        int handshakeLenIdx = payload.writerIndex();
        payload.writeZero(3);
        int handshakeStart = payload.writerIndex();
        payload.writeShort(0x0303); // TLS 1.2
        payload.writeZero(32); // Random
        payload.writeByte(0); // Session ID

        // Cipher suites: 1 GREASE + 1 real
        payload.writeShort(4);
        payload.writeShort(0x0a0a); // GREASE
        payload.writeShort(0x1301); // Real

        // Compression
        payload.writeByte(1);
        payload.writeByte(0);

        // No extensions
        payload.writeShort(0);

        // Fix handshake length
        payload.setMedium(handshakeLenIdx, payload.writerIndex() - handshakeStart);

        // Wrap in record
        ByteBuf record = Unpooled.buffer(5 + payload.readableBytes());
        record.writeByte(22);
        record.writeShort(0x0301);
        record.writeShort(payload.readableBytes());
        record.writeBytes(payload);

        TlsFingerprint fp = TlsFingerprint.fromClientHello(record);
        assertNotNull(fp);
        assertEquals(1, fp.cipherSuites().size(), "GREASE cipher should be excluded");
        assertEquals(0x1301, fp.cipherSuites().getFirst());

        payload.release();
        record.release();
    }
}
