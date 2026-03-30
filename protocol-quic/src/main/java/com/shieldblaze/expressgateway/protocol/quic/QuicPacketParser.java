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

/**
 * Zero-copy QUIC packet header parser for extracting Destination Connection IDs (DCID).
 *
 * <p>This parser reads QUIC packet headers without advancing the {@link ByteBuf#readerIndex()},
 * using only positional reads ({@code getByte()}, {@code getBytes()}, {@code getUnsignedByte()}).
 * This is critical for the L4 proxy path where the full datagram must be forwarded unmodified
 * to the backend.</p>
 *
 * <h3>QUIC Packet Header Format (RFC 9000)</h3>
 * <ul>
 *   <li><b>Long Header</b> (bit 7 of first byte = 1): Used for Initial, 0-RTT, Handshake,
 *       and Retry packets. Contains explicit DCID Length field at offset 5.</li>
 *   <li><b>Short Header</b> (bit 7 of first byte = 0): Used for 1-RTT (application data)
 *       packets. DCID immediately follows the first byte, but its length is NOT encoded
 *       in the packet -- the receiver must know it from connection state.</li>
 * </ul>
 *
 * <h3>CID Length Constraints</h3>
 * <p>RFC 9000 Section 17.2: Connection IDs MUST NOT exceed 20 bytes.</p>
 */
public final class QuicPacketParser {

    private QuicPacketParser() {
        // Utility class
    }

    /**
     * Extract the Destination Connection ID (DCID) from a QUIC packet.
     *
     * <p>For Long Header packets, the DCID length is read from the packet itself (offset 5).
     * For Short Header packets, the caller must provide the expected CID length from
     * connection state, since Short Headers do not encode the CID length.</p>
     *
     * @param buf                    the datagram content (readerIndex not advanced)
     * @param knownShortHeaderCidLen expected CID length for Short Header packets.
     *                               Use {@code -1} if unknown (Short Header extraction will be skipped).
     * @return the DCID bytes, or {@code null} if the packet is too short or unparseable
     */
    public static byte[] extractDCID(ByteBuf buf, int knownShortHeaderCidLen) {
        if (buf.readableBytes() < 1) {
            return null;
        }
        int base = buf.readerIndex();
        int firstByte = buf.getUnsignedByte(base);
        boolean isLong = (firstByte & 0x80) != 0;

        if (isLong) {
            return extractDCIDLongHeader(buf, base);
        } else {
            if (knownShortHeaderCidLen <= 0) {
                return null;
            }
            return extractDCIDShortHeader(buf, base, knownShortHeaderCidLen);
        }
    }

    /**
     * Returns {@code true} if the QUIC packet has a Long Header (bit 7 set).
     *
     * @param buf the datagram content
     * @return {@code true} for Long Header, {@code false} for Short Header or empty buffer
     */
    public static boolean isLongHeader(ByteBuf buf) {
        if (buf.readableBytes() < 1) {
            return false;
        }
        return (buf.getUnsignedByte(buf.readerIndex()) & 0x80) != 0;
    }

    /**
     * Extract the DCID Length field from a Long Header packet.
     *
     * <p>RFC 9000 Section 17.2: Long Header format is:
     * {@code [flags:1][version:4][dcid_len:1][dcid:N][scid_len:1][scid:M]...}</p>
     *
     * @param buf the datagram content
     * @return the DCID length (0..20), or {@code -1} if not a Long Header, truncated, or invalid
     */
    public static int extractDcidLength(ByteBuf buf) {
        int base = buf.readerIndex();
        if (buf.readableBytes() < 6) {
            return -1;
        }
        // Not a Long Header
        if ((buf.getUnsignedByte(base) & 0x80) == 0) {
            return -1;
        }
        int dcidLen = buf.getUnsignedByte(base + 5);
        // RFC 9000 Section 17.2: max CID length is 20 bytes
        if (dcidLen > 20) {
            return -1;
        }
        return dcidLen;
    }

    /**
     * Extract DCID from a Long Header packet.
     * Layout: [flags:1][version:4][dcid_len:1][dcid:N]...
     */
    private static byte[] extractDCIDLongHeader(ByteBuf buf, int base) {
        // Minimum: 1(flags) + 4(version) + 1(dcid_len) = 6 bytes
        if (buf.readableBytes() < 6) {
            return null;
        }
        int dcidLen = buf.getUnsignedByte(base + 5);
        if (dcidLen > 20 || buf.readableBytes() < 6 + dcidLen) {
            return null;
        }
        byte[] dcid = new byte[dcidLen];
        buf.getBytes(base + 6, dcid);
        return dcid;
    }

    /**
     * Extract DCID from a Short Header (1-RTT) packet.
     * Layout: [flags:1][dcid:N][...encrypted payload...]
     */
    private static byte[] extractDCIDShortHeader(ByteBuf buf, int base, int dcidLen) {
        if (buf.readableBytes() < 1 + dcidLen) {
            return null;
        }
        byte[] dcid = new byte[dcidLen];
        buf.getBytes(base + 1, dcid);
        return dcid;
    }
}
