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
package com.shieldblaze.expressgateway.protocol.http;

import com.shieldblaze.expressgateway.configuration.http.HttpConfiguration;
import com.shieldblaze.expressgateway.protocol.http.compression.Http11CorrectContentCompressor;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import io.netty.buffer.ByteBuf;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPipeline;
import io.netty.handler.codec.ByteToMessageDecoder;
import io.netty.handler.codec.http.HttpContentDecompressor;
import io.netty.handler.codec.http.HttpDecoderConfig;
import io.netty.handler.codec.http.HttpServerCodec;
import io.netty.handler.codec.http.HttpServerExpectContinueHandler;
import io.netty.handler.codec.http.HttpServerKeepAliveHandler;
import io.netty.handler.codec.http2.Http2Settings;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.List;

/**
 * Detects HTTP/2 prior knowledge (cleartext h2c) vs HTTP/1.1 connections
 * by inspecting the first bytes of data on a non-TLS channel.
 *
 * <p>Per RFC 9113 Section 3.4, an HTTP/2 client using prior knowledge sends
 * the connection preface immediately upon connection:
 * {@code PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n} (24 bytes).
 *
 * <p>This handler reads up to 24 bytes and checks whether they match the
 * HTTP/2 connection preface. If they do, the pipeline is reconfigured for
 * HTTP/2. Otherwise, the standard HTTP/1.1 pipeline is installed.
 *
 * <p>This approach avoids using {@code CleartextHttp2ServerUpgradeHandler}
 * and {@code HttpServerUpgradeHandler}, which intercept ALL Upgrade requests
 * (including WebSocket), breaking WebSocket proxying. Instead, h2c upgrade
 * via the {@code Upgrade: h2c} header is handled in
 * {@link Http11ServerInboundHandler} alongside WebSocket upgrade detection.
 */
public final class H2cHandler extends ByteToMessageDecoder {

    private static final Logger logger = LogManager.getLogger(H2cHandler.class);

    /**
     * HTTP/2 connection preface: "PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n"
     * RFC 9113 Section 3.4: The client connection preface starts with a
     * sequence of 24 octets.
     */
    private static final byte[] HTTP2_PREFACE = {
            0x50, 0x52, 0x49, 0x20, 0x2a, 0x20, 0x48, 0x54,
            0x54, 0x50, 0x2f, 0x32, 0x2e, 0x30, 0x0d, 0x0a,
            0x0d, 0x0a, 0x53, 0x4d, 0x0d, 0x0a, 0x0d, 0x0a
    };

    private final HTTPLoadBalancer httpLoadBalancer;

    public H2cHandler(HTTPLoadBalancer httpLoadBalancer) {
        this.httpLoadBalancer = httpLoadBalancer;
    }

    @Override
    protected void decode(ChannelHandlerContext ctx, ByteBuf in, List<Object> out) throws Exception {
        // We need at least 24 bytes to determine if this is an HTTP/2 preface.
        // If the connection starts with fewer bytes, we can still check
        // whether the available bytes match the prefix of the preface.
        // If they don't match, it's definitely HTTP/1.1.
        int readableBytes = in.readableBytes();

        if (readableBytes == 0) {
            return;
        }

        // Check available bytes against the preface prefix. If any byte
        // doesn't match, it's definitely not HTTP/2 prior knowledge.
        int bytesToCheck = Math.min(readableBytes, HTTP2_PREFACE.length);
        int readerIndex = in.readerIndex();
        for (int i = 0; i < bytesToCheck; i++) {
            if (in.getByte(readerIndex + i) != HTTP2_PREFACE[i]) {
                // Definitely HTTP/1.1 -- configure the pipeline
                configureHttp1Pipeline(ctx);
                return;
            }
        }

        // If we've matched all bytes so far but don't have the full 24 bytes yet,
        // wait for more data.
        if (readableBytes < HTTP2_PREFACE.length) {
            return;
        }

        // Full 24-byte preface matched -- this is HTTP/2 prior knowledge.
        configureHttp2Pipeline(ctx);
    }

    /**
     * Configure the pipeline for HTTP/2 (prior knowledge, cleartext).
     * Replaces this handler with the H2 codec and H2 inbound handler.
     */
    private void configureHttp2Pipeline(ChannelHandlerContext ctx) {
        logger.debug("HTTP/2 connection preface detected (prior knowledge h2c)");

        ChannelPipeline pipeline = ctx.pipeline();
        HttpConfiguration httpConfiguration = httpLoadBalancer.httpConfiguration();

        Http2Settings http2Settings = Http2Settings.defaultSettings()
                .maxConcurrentStreams(httpConfiguration.maxConcurrentStreams())
                .initialWindowSize(httpConfiguration.initialWindowSize());

        // Add the H2 codec and handler. Do NOT consume the preface bytes -- the
        // H2 codec needs to read them to complete its own connection preface handling.
        pipeline.addAfter(ctx.name(), "h2codec",
                CompressibleHttp2FrameCodec.forServer(httpLoadBalancer.compressionOptions())
                        .maxHeaderListSize(httpConfiguration.maxHeaderListSize())
                        .initialSettings(http2Settings)
                        .build());
        // RFC 9113 Section 6.9.2: Increase connection-level flow control window
        // from the default 65535 to the configured value. This handler sends a
        // WINDOW_UPDATE on stream 0 and then removes itself from the pipeline.
        pipeline.addAfter("h2codec", "h2connWindow",
                new H2ConnectionWindowHandler(httpConfiguration.h2ConnectionWindowSize()));
        pipeline.addAfter("h2connWindow", "h2handler",
                new Http2ServerInboundHandler(httpLoadBalancer, false));

        // Remove this handler -- the H2 codec will handle the rest.
        pipeline.remove(this);
    }

    /**
     * Configure the pipeline for HTTP/1.1. Adds the standard HTTP/1.1
     * handler chain and removes this handler. The buffered bytes are
     * passed through to the HTTP codec.
     */
    private void configureHttp1Pipeline(ChannelHandlerContext ctx) {
        logger.debug("HTTP/1.1 connection detected (not HTTP/2 prior knowledge)");

        ChannelPipeline pipeline = ctx.pipeline();
        HttpConfiguration httpConfiguration = httpLoadBalancer.httpConfiguration();

        // Insert the full HTTP/1.1 pipeline after this handler, then remove ourselves.
        // The bytes already buffered in the ByteToMessageDecoder's cumulation buffer
        // will be passed to the next handler automatically by Netty's
        // ByteToMessageDecoder.handlerRemoved() -> channelRead() path.
        pipeline.addAfter(ctx.name(), "httpCodec", new HttpServerCodec(new HttpDecoderConfig()
                .setMaxInitialLineLength(httpConfiguration.maxInitialLineLength())
                .setMaxHeaderSize(httpConfiguration.maxHeaderSize())
                .setMaxChunkSize(httpConfiguration.maxChunkSize())
                .setValidateHeaders(true)
        ));
        // SEC-02: Slowloris defense — enforce a deadline for receiving the first
        // complete request headers. Must sit after HttpServerCodec (which decodes
        // raw bytes into HttpRequest) so this handler sees decoded HTTP messages.
        // handlerAdded() detects the channel is already active and starts the timer.
        pipeline.addAfter("httpCodec", "headerTimeout",
                new RequestHeaderTimeoutHandler(httpConfiguration.requestHeaderTimeoutSeconds()));
        pipeline.addAfter("headerTimeout", "expectContinue", new HttpServerExpectContinueHandler());
        pipeline.addAfter("expectContinue", "keepAlive", new HttpServerKeepAliveHandler());
        pipeline.addAfter("keepAlive", "compressor",
                new Http11CorrectContentCompressor(httpConfiguration.compressionThreshold(), httpLoadBalancer.compressionOptions()));
        pipeline.addAfter("compressor", "decompressor", new HttpContentDecompressor(0));
        pipeline.addAfter("decompressor", "bodySizeLimit",
                new RequestBodySizeLimitHandler(httpConfiguration.maxRequestBodySize()));
        // ME-04: Slow-POST defense for cleartext H1 path
        String afterBodyLimit = "bodySizeLimit";
        if (httpConfiguration.requestBodyTimeoutSeconds() > 0) {
            pipeline.addAfter(afterBodyLimit, "bodyTimeout",
                    new RequestBodyTimeoutHandler(httpConfiguration.requestBodyTimeoutSeconds()));
            afterBodyLimit = "bodyTimeout";
        }
        pipeline.addAfter(afterBodyLimit, "accessLog", new AccessLogHandler());
        pipeline.addAfter("accessLog", "http11Handler",
                new Http11ServerInboundHandler(httpLoadBalancer, false));

        // Remove this handler. ByteToMessageDecoder.handlerRemoved() will fire
        // any buffered bytes into the newly configured pipeline.
        pipeline.remove(this);
    }
}
