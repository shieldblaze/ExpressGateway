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

import com.shieldblaze.expressgateway.configuration.transport.BackendProxyProtocolMode;
import io.netty.buffer.ByteBuf;
import io.netty.channel.Channel;
import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;
import lombok.extern.log4j.Log4j2;

import java.net.Inet4Address;
import java.net.Inet6Address;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.util.Objects;

/**
 * One-shot outbound encoder that sends a HAProxy PROXY protocol header as the
 * first bytes on a new backend connection.
 *
 * <h3>Ordering guarantee</h3>
 * <p>This handler overrides {@link #write(ChannelHandlerContext, Object, ChannelPromise)}
 * (outbound) to prepend the PP header to the very first outbound write. This is
 * critical for correctness: Netty's NIO connect path notifies the connect-future
 * listeners (which drain the backlog) <em>before</em> firing {@code channelActive}
 * through the pipeline. If the PP header were written in {@code channelActive}, any
 * backlog data that the client sent before the backend TCP connect completed would
 * arrive at the backend <em>before</em> the PP header. Intercepting the first
 * {@code write()} call guarantees the PP header always leads, regardless of timing.</p>
 *
 * <h3>TLS compatibility</h3>
 * <p>When TLS is configured, {@code SslHandler} sits after this handler in the
 * pipeline. The SslHandler's TLS ClientHello is sent as an outbound write triggered
 * by {@code channelActive}. By intercepting the first {@code write()}, the PP header
 * is injected immediately before the TLS ClientHello — ensuring it arrives as raw
 * TCP bytes preceding the TLS record, which is exactly what the PROXY protocol
 * specification requires.</p>
 *
 * <h3>One-shot removal</h3>
 * <p>After prepending the PP header to the first write, this handler removes itself
 * from the pipeline. It is never invoked again for subsequent writes on the same
 * connection.</p>
 *
 * <p>Supports both v1 (text) and v2 (binary) encoding.</p>
 *
 * <p>Reference: <a href="https://www.haproxy.org/download/2.9/doc/proxy-protocol.txt">
 * PROXY Protocol Specification</a></p>
 */
@Log4j2
public final class ProxyProtocolEncoder extends ChannelDuplexHandler {

    /**
     * PROXY protocol v2 binary signature (12 bytes)
     */
    private static final byte[] V2_SIGNATURE = {
            0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A
    };

    private static final int V2_HEADER_LEN = 16; // 12 signature + 4 (ver/cmd, fam, len)
    private static final int V2_ADDR_LEN_IPV4 = 12; // 4+4+2+2
    private static final int V2_ADDR_LEN_IPV6 = 36; // 16+16+2+2

    private final BackendProxyProtocolMode mode;
    private final Channel frontendChannel;

    /**
     * Encoded PP header bytes, computed eagerly at construction time so that
     * {@link #write} can inject them without needing to allocate or inspect
     * address info again. The {@code byte[]} form avoids holding a {@link ByteBuf}
     * reference before the channel is active (which would require ref-counting
     * across handler lifecycle).
     */
    private final byte[] headerBytes;

    /**
     * @param mode            v1 or v2 encoding mode (must not be OFF)
     * @param frontendChannel the client-facing channel whose address info is encoded
     */
    public ProxyProtocolEncoder(BackendProxyProtocolMode mode, Channel frontendChannel) {
        this.mode = Objects.requireNonNull(mode, "BackendProxyProtocolMode");
        this.frontendChannel = Objects.requireNonNull(frontendChannel, "frontendChannel");
        if (mode == BackendProxyProtocolMode.OFF) {
            throw new IllegalArgumentException("ProxyProtocolEncoder should not be created when mode is OFF");
        }
        // Compute the header bytes eagerly at construction time. The frontend channel's
        // addresses are available immediately (the client connection is already active).
        this.headerBytes = computeHeaderBytes(mode, frontendChannel);
    }

    /**
     * Intercepts the first outbound write to prepend the PP header, then removes
     * this handler from the pipeline.
     *
     * <p>The PP header is written as a separate {@link ByteBuf} immediately before
     * the original message. Both are queued in the channel's outbound buffer before
     * the next flush — guaranteeing the header leads any application data.</p>
     */
    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) throws Exception {
        // Allocate and write the PP header immediately before the original message.
        // Use the channel's allocator (pool-backed) to avoid off-heap fragmentation.
        ByteBuf header = ctx.alloc().buffer(headerBytes.length);
        header.writeBytes(headerBytes);
        ctx.write(header); // header write uses a void promise — failure is non-fatal here
                           // (the channel will close on the next write failure anyway)

        // Remove self before writing the original message so subsequent writes skip
        // this handler entirely. Must be done before ctx.write(msg) to avoid
        // re-entrant invocation of this method.
        ctx.pipeline().remove(this);

        // Write the original message through the (now-modified) pipeline.
        ctx.write(msg, promise);
    }

    // ---- Header computation ----

    /**
     * Computes the PP header bytes from the frontend channel's address information.
     * Called once at construction time; the result is stored in {@link #headerBytes}.
     */
    private static byte[] computeHeaderBytes(BackendProxyProtocolMode mode, Channel frontendChannel) {
        InetSocketAddress srcAddr = resolveSourceAddress(frontendChannel);
        InetSocketAddress dstAddr = resolveDestinationAddress(frontendChannel);

        return switch (mode) {
            case V1  -> encodeV1Bytes(srcAddr, dstAddr);
            case V2  -> encodeV2Bytes(srcAddr, dstAddr);
            case OFF -> throw new IllegalStateException("OFF mode should not reach encoder");
        };
    }

    private static InetSocketAddress resolveSourceAddress(Channel frontendChannel) {
        // Prefer the real client address decoded from an inbound PROXY protocol header
        InetSocketAddress realAddr = frontendChannel.attr(ProxyProtocolHandler.REAL_CLIENT_ADDRESS).get();
        if (realAddr != null) {
            return realAddr;
        }
        // Fall back to the direct TCP peer address
        if (frontendChannel.remoteAddress() instanceof InetSocketAddress addr) {
            return addr;
        }
        return null;
    }

    private static InetSocketAddress resolveDestinationAddress(Channel frontendChannel) {
        if (frontendChannel.localAddress() instanceof InetSocketAddress addr) {
            return addr;
        }
        return null;
    }

    /**
     * Normalize an InetAddress: if it is an IPv4-mapped IPv6 address (::ffff:x.x.x.x),
     * extract and return the underlying IPv4 address. Otherwise return as-is.
     */
    private static InetAddress normalizeAddress(InetAddress addr) {
        if (addr instanceof Inet6Address) {
            byte[] bytes = addr.getAddress();
            // IPv4-mapped IPv6: first 10 bytes zero, bytes 10-11 are 0xFF
            if (bytes.length == 16) {
                boolean mapped = true;
                for (int i = 0; i < 10; i++) {
                    if (bytes[i] != 0) {
                        mapped = false;
                        break;
                    }
                }
                if (mapped && bytes[10] == (byte) 0xFF && bytes[11] == (byte) 0xFF) {
                    try {
                        return Inet4Address.getByAddress(new byte[]{bytes[12], bytes[13], bytes[14], bytes[15]});
                    } catch (Exception e) {
                        // Fall through to return original
                    }
                }
            }
        }
        return addr;
    }

    // ---- V1 Encoding ----

    private static byte[] encodeV1Bytes(InetSocketAddress src, InetSocketAddress dst) {
        if (src == null || dst == null || src.isUnresolved() || dst.isUnresolved()) {
            return "PROXY UNKNOWN\r\n".getBytes(StandardCharsets.US_ASCII);
        }

        InetAddress srcInet = normalizeAddress(src.getAddress());
        InetAddress dstInet = normalizeAddress(dst.getAddress());

        // Both must be the same family for the header; if mixed, use UNKNOWN
        boolean srcV4 = srcInet instanceof Inet4Address;
        boolean dstV4 = dstInet instanceof Inet4Address;
        if (srcV4 != dstV4) {
            return "PROXY UNKNOWN\r\n".getBytes(StandardCharsets.US_ASCII);
        }

        String family = srcV4 ? "TCP4" : "TCP6";
        String line = "PROXY " + family + " " +
                srcInet.getHostAddress() + " " +
                dstInet.getHostAddress() + " " +
                src.getPort() + " " +
                dst.getPort() + "\r\n";

        return line.getBytes(StandardCharsets.US_ASCII);
    }

    // ---- V2 Encoding ----

    private static byte[] encodeV2Bytes(InetSocketAddress src, InetSocketAddress dst) {
        if (src == null || dst == null || src.isUnresolved() || dst.isUnresolved()) {
            return encodeV2LocalBytes();
        }

        InetAddress srcInet = normalizeAddress(src.getAddress());
        InetAddress dstInet = normalizeAddress(dst.getAddress());

        boolean srcV4 = srcInet instanceof Inet4Address;
        boolean dstV4 = dstInet instanceof Inet4Address;
        if (srcV4 != dstV4) {
            return encodeV2LocalBytes();
        }

        if (srcV4) {
            return encodeV2Ipv4Bytes(srcInet, src.getPort(), dstInet, dst.getPort());
        } else {
            return encodeV2Ipv6Bytes(srcInet, src.getPort(), dstInet, dst.getPort());
        }
    }

    private static byte[] encodeV2Ipv4Bytes(InetAddress src, int srcPort,
                                              InetAddress dst, int dstPort) {
        byte[] buf = new byte[V2_HEADER_LEN + V2_ADDR_LEN_IPV4];
        int pos = 0;

        // 12-byte signature
        System.arraycopy(V2_SIGNATURE, 0, buf, pos, V2_SIGNATURE.length);
        pos += V2_SIGNATURE.length;

        buf[pos++] = 0x21; // Version 2, PROXY command
        buf[pos++] = 0x11; // AF_INET, STREAM
        buf[pos++] = 0;    // addrLen high byte (12 = 0x000C)
        buf[pos++] = 12;   // addrLen low byte

        // Src IP (4 bytes)
        byte[] srcIp = src.getAddress();
        System.arraycopy(srcIp, 0, buf, pos, 4);
        pos += 4;

        // Dst IP (4 bytes)
        byte[] dstIp = dst.getAddress();
        System.arraycopy(dstIp, 0, buf, pos, 4);
        pos += 4;

        // Src port (2 bytes big-endian)
        buf[pos++] = (byte) (srcPort >>> 8);
        buf[pos++] = (byte)  srcPort;

        // Dst port (2 bytes big-endian)
        buf[pos++] = (byte) (dstPort >>> 8);
        buf[pos]   = (byte)  dstPort;

        return buf;
    }

    private static byte[] encodeV2Ipv6Bytes(InetAddress src, int srcPort,
                                              InetAddress dst, int dstPort) {
        byte[] buf = new byte[V2_HEADER_LEN + V2_ADDR_LEN_IPV6];
        int pos = 0;

        System.arraycopy(V2_SIGNATURE, 0, buf, pos, V2_SIGNATURE.length);
        pos += V2_SIGNATURE.length;

        buf[pos++] = 0x21; // Version 2, PROXY command
        buf[pos++] = 0x21; // AF_INET6, STREAM
        buf[pos++] = 0;    // addrLen high byte (36 = 0x0024)
        buf[pos++] = 36;   // addrLen low byte

        byte[] srcIp = src.getAddress();
        System.arraycopy(srcIp, 0, buf, pos, 16);
        pos += 16;

        byte[] dstIp = dst.getAddress();
        System.arraycopy(dstIp, 0, buf, pos, 16);
        pos += 16;

        buf[pos++] = (byte) (srcPort >>> 8);
        buf[pos++] = (byte)  srcPort;

        buf[pos++] = (byte) (dstPort >>> 8);
        buf[pos]   = (byte)  dstPort;

        return buf;
    }

    /**
     * Encode a v2 LOCAL command header (no address information).
     * Used when the real client address cannot be determined.
     */
    private static byte[] encodeV2LocalBytes() {
        byte[] buf = new byte[V2_HEADER_LEN];
        int pos = 0;
        System.arraycopy(V2_SIGNATURE, 0, buf, pos, V2_SIGNATURE.length);
        pos += V2_SIGNATURE.length;
        buf[pos++] = 0x20; // Version 2, LOCAL command
        buf[pos++] = 0x00; // AF_UNSPEC, UNSPEC
        buf[pos++] = 0;    // addrLen = 0 (high byte)
        buf[pos]   = 0;    // addrLen = 0 (low byte)
        return buf;
    }
}
