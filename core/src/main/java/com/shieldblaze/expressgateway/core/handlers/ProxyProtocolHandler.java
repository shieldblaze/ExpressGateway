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
package com.shieldblaze.expressgateway.core.handlers;

import com.shieldblaze.expressgateway.configuration.transport.ProxyProtocolMode;
import io.netty.buffer.ByteBuf;
import io.netty.channel.ChannelHandlerContext;
import io.netty.handler.codec.ByteToMessageDecoder;
import io.netty.util.AttributeKey;
import lombok.extern.log4j.Log4j2;

import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.util.List;

import static com.shieldblaze.expressgateway.common.utils.LogSanitizer.sanitize;
import java.util.Objects;

/**
 * Decodes HAProxy PROXY protocol v1 (text) and v2 (binary) headers
 * to extract the real client address behind upstream proxies/load balancers.
 *
 * <p>The real client address is stored in channel attributes under
 * {@link #REAL_CLIENT_ADDRESS} for downstream handlers to access.</p>
 *
 * <p>Reference: <a href="https://www.haproxy.org/download/2.9/doc/proxy-protocol.txt">
 * PROXY Protocol Specification</a></p>
 *
 * <p>This handler removes itself from the pipeline after decoding the
 * PROXY protocol header (one-shot decoder).</p>
 */
@Log4j2
public final class ProxyProtocolHandler extends ByteToMessageDecoder {

    /**
     * Channel attribute key for the real client address extracted from the PROXY protocol header.
     */
    public static final AttributeKey<InetSocketAddress> REAL_CLIENT_ADDRESS =
            AttributeKey.valueOf("PROXY_PROTOCOL_REAL_CLIENT_ADDRESS");

    /**
     * PROXY protocol v2 binary signature (12 bytes):
     * \x0D\x0A\x0D\x0A\x00\x0D\x0A\x51\x55\x49\x54\x0A
     */
    private static final byte[] V2_SIGNATURE = {
            0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A
    };

    /**
     * PROXY protocol v1 text prefix
     */
    private static final byte[] V1_PREFIX = "PROXY ".getBytes(StandardCharsets.US_ASCII);

    /**
     * Maximum length for v1 header (107 bytes per spec, plus slack)
     */
    private static final int V1_MAX_LENGTH = 108;

    /**
     * v2 fixed header length: 12 (signature) + 4 (ver/cmd, fam, len)
     */
    private static final int V2_HEADER_LENGTH = 16;

    private final ProxyProtocolMode mode;

    public ProxyProtocolHandler(ProxyProtocolMode mode) {
        this.mode = Objects.requireNonNull(mode, "ProxyProtocolMode");
        if (mode == ProxyProtocolMode.OFF) {
            throw new IllegalArgumentException("ProxyProtocolHandler should not be added when mode is OFF");
        }
    }

    @Override
    protected void decode(ChannelHandlerContext ctx, ByteBuf in, List<Object> out) throws Exception {
        // Need at least enough bytes to distinguish v1 from v2
        if (in.readableBytes() < Math.max(V1_PREFIX.length, V2_SIGNATURE.length)) {
            return; // Wait for more data
        }

        int readerIndex = in.readerIndex();

        if (mode == ProxyProtocolMode.AUTO) {
            if (matchesV2Signature(in, readerIndex)) {
                decodeV2(ctx, in);
            } else if (matchesV1Prefix(in, readerIndex)) {
                decodeV1(ctx, in);
            } else {
                log.error("PROXY protocol: unrecognized header, closing connection");
                ctx.close();
            }
        } else if (mode == ProxyProtocolMode.V1) {
            if (matchesV1Prefix(in, readerIndex)) {
                decodeV1(ctx, in);
            } else {
                log.error("PROXY protocol v1 expected but not found, closing connection");
                ctx.close();
            }
        } else if (mode == ProxyProtocolMode.V2) {
            if (matchesV2Signature(in, readerIndex)) {
                decodeV2(ctx, in);
            } else {
                log.error("PROXY protocol v2 expected but not found, closing connection");
                ctx.close();
            }
        }
    }

    /**
     * Decode PROXY protocol v1 text format.
     * Format: {@code PROXY TCP4|TCP6|UNKNOWN srcIP dstIP srcPort dstPort\r\n}
     */
    private void decodeV1(ChannelHandlerContext ctx, ByteBuf in) {
        // Find the \r\n terminator
        int readerIndex = in.readerIndex();
        int readableBytes = in.readableBytes();
        int lineEnd = -1;

        int searchLimit = Math.min(readableBytes, V1_MAX_LENGTH);
        for (int i = 0; i < searchLimit - 1; i++) {
            if (in.getByte(readerIndex + i) == '\r' && in.getByte(readerIndex + i + 1) == '\n') {
                lineEnd = i;
                break;
            }
        }

        if (lineEnd < 0) {
            if (readableBytes >= V1_MAX_LENGTH) {
                // Header too long -- protocol violation
                log.error("PROXY protocol v1: header exceeds maximum length, closing connection");
                ctx.close();
            }
            // Otherwise wait for more data (incomplete header)
            return;
        }

        // Read the full header line (excluding \r\n)
        byte[] headerBytes = new byte[lineEnd];
        in.getBytes(readerIndex, headerBytes);
        in.skipBytes(lineEnd + 2); // Skip header + \r\n

        String header = new String(headerBytes, StandardCharsets.US_ASCII);
        String[] parts = header.split(" ");

        // parts[0] = "PROXY", parts[1] = protocol, parts[2] = srcIP,
        // parts[3] = dstIP, parts[4] = srcPort, parts[5] = dstPort
        if (parts.length >= 6 && !parts[1].equals("UNKNOWN")) {
            try {
                String srcIp = parts[2];
                int srcPort = Integer.parseInt(parts[4]);
                InetSocketAddress realAddress = new InetSocketAddress(srcIp, srcPort);

                ctx.channel().attr(REAL_CLIENT_ADDRESS).set(realAddress);
                log.debug("PROXY v1: real client address = {}", sanitize(realAddress.toString()));
            } catch (Exception ex) {
                log.warn("PROXY protocol v1: failed to parse header '{}', ignoring", sanitize(header), ex);
            }
        } else if (parts.length >= 2 && parts[1].equals("UNKNOWN")) {
            // UNKNOWN means the connection was not initiated from a known source
            log.debug("PROXY v1: UNKNOWN protocol family, no real address extracted");
        }

        // Remove this handler from the pipeline -- one-shot decode
        ctx.pipeline().remove(this);
    }

    /**
     * Decode PROXY protocol v2 binary format.
     *
     * <pre>
     * Byte 0-11:  Signature
     * Byte 12:    Version (high nibble) | Command (low nibble)
     * Byte 13:    Address family (high nibble) | Transport protocol (low nibble)
     * Byte 14-15: Address length (network byte order)
     * Byte 16+:   Address data
     * </pre>
     */
    private void decodeV2(ChannelHandlerContext ctx, ByteBuf in) {
        if (in.readableBytes() < V2_HEADER_LENGTH) {
            return; // Wait for more data
        }

        int readerIndex = in.readerIndex();

        // Read version and command (byte 12)
        int verCmd = in.getUnsignedByte(readerIndex + 12);
        int version = (verCmd >> 4) & 0x0F;
        int command = verCmd & 0x0F;

        if (version != 2) {
            log.error("PROXY protocol v2: unsupported version {}, closing connection", version);
            ctx.close();
            return;
        }

        // Read family (byte 13)
        int familyByte = in.getUnsignedByte(readerIndex + 13);
        int addressFamily = (familyByte >> 4) & 0x0F;

        // Read address length (bytes 14-15, big-endian)
        int addrLen = in.getUnsignedShort(readerIndex + 14);

        // Total header size = 16 (fixed) + addrLen
        int totalLen = V2_HEADER_LENGTH + addrLen;
        if (in.readableBytes() < totalLen) {
            return; // Wait for more data
        }

        if (command == 0x00) {
            // LOCAL command -- health check / internal, no address to extract
            log.debug("PROXY v2: LOCAL command, no address extracted");
        } else if (command == 0x01) {
            // PROXY command -- extract source address
            if (addressFamily == 0x01) {
                // AF_INET (IPv4): 4 bytes src, 4 bytes dst, 2 bytes src port, 2 bytes dst port = 12 bytes
                if (addrLen >= 12) {
                    int offset = readerIndex + V2_HEADER_LENGTH;
                    byte[] srcAddr = new byte[4];
                    in.getBytes(offset, srcAddr);
                    int srcPort = in.getUnsignedShort(offset + 8);

                    String srcIp = (srcAddr[0] & 0xFF) + "." + (srcAddr[1] & 0xFF) + "." +
                            (srcAddr[2] & 0xFF) + "." + (srcAddr[3] & 0xFF);
                    InetSocketAddress realAddress = new InetSocketAddress(srcIp, srcPort);
                    ctx.channel().attr(REAL_CLIENT_ADDRESS).set(realAddress);
                    log.debug("PROXY v2: real client address (IPv4) = {}", realAddress);
                }
            } else if (addressFamily == 0x02) {
                // AF_INET6 (IPv6): 16 bytes src, 16 bytes dst, 2 bytes src port, 2 bytes dst port = 36 bytes
                if (addrLen >= 36) {
                    int offset = readerIndex + V2_HEADER_LENGTH;
                    byte[] srcAddr = new byte[16];
                    in.getBytes(offset, srcAddr);
                    int srcPort = in.getUnsignedShort(offset + 32);

                    StringBuilder sb = new StringBuilder();
                    for (int i = 0; i < 16; i += 2) {
                        if (i > 0) sb.append(':');
                        sb.append(String.format("%02x%02x", srcAddr[i] & 0xFF, srcAddr[i + 1] & 0xFF));
                    }
                    InetSocketAddress realAddress = new InetSocketAddress(sb.toString(), srcPort);
                    ctx.channel().attr(REAL_CLIENT_ADDRESS).set(realAddress);
                    log.debug("PROXY v2: real client address (IPv6) = {}", realAddress);
                }
            } else if (addressFamily == 0x00) {
                // AF_UNSPEC -- no address
                log.debug("PROXY v2: UNSPEC address family, no address extracted");
            }
        }

        // Skip entire PROXY protocol header
        in.skipBytes(totalLen);

        // Remove this handler from the pipeline -- one-shot decode
        ctx.pipeline().remove(this);
    }

    /**
     * Check if the buffer at the given index starts with the v2 binary signature
     */
    private static boolean matchesV2Signature(ByteBuf buf, int readerIndex) {
        if (buf.readableBytes() < V2_SIGNATURE.length) {
            return false;
        }
        for (int i = 0; i < V2_SIGNATURE.length; i++) {
            if (buf.getByte(readerIndex + i) != V2_SIGNATURE[i]) {
                return false;
            }
        }
        return true;
    }

    /**
     * Check if the buffer at the given index starts with "PROXY "
     */
    private static boolean matchesV1Prefix(ByteBuf buf, int readerIndex) {
        if (buf.readableBytes() < V1_PREFIX.length) {
            return false;
        }
        for (int i = 0; i < V1_PREFIX.length; i++) {
            if (buf.getByte(readerIndex + i) != V1_PREFIX[i]) {
                return false;
            }
        }
        return true;
    }
}
