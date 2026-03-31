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
package com.shieldblaze.expressgateway.protocol.quic;

import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * RB-TEST-01 (supplementary): Tests for {@link QuicPacketParser} -- DCID extraction
 * from QUIC Long Header and Short Header packets per RFC 9000 Section 17.
 *
 * <p>These tests verify the zero-copy CID extraction logic used for CID-based
 * load balancing and connection migration routing.</p>
 */
@Timeout(value = 10)
class QuicPacketParserTest {

    // -- Long Header tests (RFC 9000 Section 17.2) --

    @Test
    void longHeader_extractDCID_8bytesCid() {
        // Long Header: first byte has bit 7 set
        // Format: [flags:1][version:4][dcid_len:1][dcid:N][scid_len:1][scid:M]...
        byte[] dcid = {0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08};
        ByteBuf buf = buildLongHeaderPacket(dcid);

        byte[] extracted = QuicPacketParser.extractDCID(buf, -1);
        assertNotNull(extracted, "Must extract DCID from Long Header");
        assertArrayEquals(dcid, extracted);
        assertEquals(0, buf.readerIndex(), "readerIndex must not be advanced (zero-copy)");

        buf.release();
    }

    @Test
    void longHeader_extractDCID_20bytesMaxCid() {
        // RFC 9000 Section 17.2: max CID length is 20 bytes
        byte[] dcid = new byte[20];
        for (int i = 0; i < 20; i++) dcid[i] = (byte) (i + 0x10);
        ByteBuf buf = buildLongHeaderPacket(dcid);

        byte[] extracted = QuicPacketParser.extractDCID(buf, -1);
        assertNotNull(extracted);
        assertArrayEquals(dcid, extracted);

        buf.release();
    }

    @Test
    void longHeader_extractDCID_zeroByteCid() {
        byte[] dcid = new byte[0]; // Empty DCID is valid per RFC 9000
        ByteBuf buf = buildLongHeaderPacket(dcid);

        byte[] extracted = QuicPacketParser.extractDCID(buf, -1);
        assertNotNull(extracted);
        assertEquals(0, extracted.length);

        buf.release();
    }

    @Test
    void longHeader_extractDCID_cidLengthExceeds20_returnsNull() {
        // CID length > 20 is invalid per RFC 9000
        ByteBuf buf = Unpooled.buffer(7);
        buf.writeByte(0xC0); // Long Header flag
        buf.writeInt(0x00000001); // Version
        buf.writeByte(21); // Invalid CID length
        buf.writeByte(0x00); // Filler

        assertNull(QuicPacketParser.extractDCID(buf, -1), "CID length > 20 must return null");

        buf.release();
    }

    @Test
    void longHeader_truncatedPacket_returnsNull() {
        // Packet too short to contain flags + version + dcid_len (6 bytes)
        ByteBuf buf = Unpooled.wrappedBuffer(new byte[]{(byte) 0xC0, 0x00, 0x00});
        assertNull(QuicPacketParser.extractDCID(buf, -1), "Truncated packet must return null");
        buf.release();
    }

    @Test
    void longHeader_packetShorterThanDcidLen_returnsNull() {
        // Header says DCID is 8 bytes but only 4 bytes follow
        ByteBuf buf = Unpooled.buffer(10);
        buf.writeByte(0xC0); // Long Header
        buf.writeInt(0x00000001); // Version
        buf.writeByte(8); // DCID length = 8
        buf.writeBytes(new byte[]{1, 2, 3, 4}); // Only 4 bytes of DCID

        assertNull(QuicPacketParser.extractDCID(buf, -1), "Packet shorter than DCID length must return null");

        buf.release();
    }

    @Test
    void isLongHeader_bitCheck() {
        ByteBuf longHeader = Unpooled.wrappedBuffer(new byte[]{(byte) 0xC0});
        assertTrue(QuicPacketParser.isLongHeader(longHeader));
        longHeader.release();

        ByteBuf shortHeader = Unpooled.wrappedBuffer(new byte[]{0x40});
        assertFalse(QuicPacketParser.isLongHeader(shortHeader));
        shortHeader.release();

        ByteBuf empty = Unpooled.buffer(0);
        assertFalse(QuicPacketParser.isLongHeader(empty));
        empty.release();
    }

    @Test
    void extractDcidLength_validLongHeader() {
        byte[] dcid = {0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08};
        ByteBuf buf = buildLongHeaderPacket(dcid);

        assertEquals(8, QuicPacketParser.extractDcidLength(buf));

        buf.release();
    }

    @Test
    void extractDcidLength_shortHeader_returnsMinusOne() {
        ByteBuf buf = Unpooled.wrappedBuffer(new byte[]{0x40, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06});
        assertEquals(-1, QuicPacketParser.extractDcidLength(buf));
        buf.release();
    }

    // -- Short Header tests (RFC 9000 Section 17.3) --

    @Test
    void shortHeader_extractDCID_knownLength() {
        // Short Header: first byte has bit 7 clear
        // Format: [flags:1][dcid:N][encrypted payload...]
        byte[] dcid = {0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11};
        ByteBuf buf = Unpooled.buffer(1 + dcid.length + 16);
        buf.writeByte(0x40); // Short Header (bit 7 = 0, bit 6 = 1 for fixed bit)
        buf.writeBytes(dcid);
        buf.writeBytes(new byte[16]); // Encrypted payload

        byte[] extracted = QuicPacketParser.extractDCID(buf, 8);
        assertNotNull(extracted, "Must extract DCID from Short Header with known length");
        assertArrayEquals(dcid, extracted);
        assertEquals(0, buf.readerIndex(), "readerIndex must not be advanced");

        buf.release();
    }

    @Test
    void shortHeader_unknownLength_returnsNull() {
        ByteBuf buf = Unpooled.wrappedBuffer(new byte[]{0x40, 0x01, 0x02, 0x03, 0x04});

        // knownShortHeaderCidLen = -1 means unknown
        assertNull(QuicPacketParser.extractDCID(buf, -1),
                "Short Header with unknown CID length must return null");
        // knownShortHeaderCidLen = 0 also means unknown
        assertNull(QuicPacketParser.extractDCID(buf, 0));

        buf.release();
    }

    @Test
    void shortHeader_truncatedPacket_returnsNull() {
        // Packet has 2 bytes after flags, but we claim CID is 8 bytes
        ByteBuf buf = Unpooled.wrappedBuffer(new byte[]{0x40, 0x01, 0x02});
        assertNull(QuicPacketParser.extractDCID(buf, 8), "Truncated Short Header must return null");
        buf.release();
    }

    @Test
    void emptyBuffer_returnsNull() {
        ByteBuf empty = Unpooled.buffer(0);
        assertNull(QuicPacketParser.extractDCID(empty, 8));
        empty.release();
    }

    // -- Helper --

    private ByteBuf buildLongHeaderPacket(byte[] dcid) {
        // Build a minimal valid QUIC Long Header packet
        ByteBuf buf = Unpooled.buffer(6 + dcid.length + 32);
        buf.writeByte(0xC0); // Long Header (Initial packet type)
        buf.writeInt(0x00000001); // Version 1
        buf.writeByte(dcid.length); // DCID Length
        buf.writeBytes(dcid); // DCID
        buf.writeByte(0); // SCID Length = 0
        buf.writeBytes(new byte[32]); // Remaining packet data
        return buf;
    }
}
