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

import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.LastHttpContent;
import io.netty.handler.ssl.SslHandler;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import javax.net.ssl.SSLSession;

/**
 * Access log handler that records HTTP request/response metadata in structured
 * JSON format for log aggregation compatibility (e.g., ELK, Loki, Datadog).
 *
 * <p>Each completed request/response cycle emits a single JSON line:
 * <pre>
 * {"method":"GET","uri":"/path","status":200,"latencyMs":3,"bytes":128,"requestId":"abc-123"}
 * </pre>
 *
 * <p>Uses a dedicated "accesslog" logger so operators can route access logs
 * independently of application logs (e.g., to a separate file or pipeline).
 * JSON formatting is done via manual StringBuilder to avoid Jackson allocation
 * on the hot path. URI and requestId values are JSON-escaped to handle
 * special characters safely.</p>
 *
 * <p>Thread safety: each channel gets its own handler instance
 * (not {@code @Sharable}), so instance fields are confined to
 * a single event-loop thread — no synchronization required.</p>
 */
public final class AccessLogHandler extends ChannelDuplexHandler {

    private static final Logger ACCESS_LOG = LogManager.getLogger("accesslog");

    // Per-request state — confined to the channel's event-loop thread
    private String method;
    private String uri;
    private String requestId;
    private long requestStartNanos;
    private int statusCode;
    private long responseBytes;

    // ME-06: Additional fields for production observability
    private String protocol;   // "HTTP/1.1" or "HTTP/2"
    private String tlsCipher;  // TLS cipher suite name or null for cleartext

    @Override
    public void channelActive(ChannelHandlerContext ctx) throws Exception {
        // ME-06: Detect protocol and TLS cipher once per connection.
        // These don't change during the connection lifetime.
        protocol = "HTTP/1.1"; // This handler is only used in H1 pipeline
        SslHandler sslHandler = ctx.pipeline().get(SslHandler.class);
        if (sslHandler != null) {
            SSLSession session = sslHandler.engine().getSession();
            tlsCipher = session.getCipherSuite();
        }
        super.channelActive(ctx);
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof HttpRequest request) {
            method = request.method().name();
            uri = request.uri();
            // Capture X-Request-ID for correlation in access logs. At this point in the
            // pipeline the header may not yet be injected by Http11ServerInboundHandler
            // (which runs after this handler in channelRead order). However, if the client
            // sent one, capture it here. The downstream handler will log it on response.
            requestId = request.headers().get(Headers.X_REQUEST_ID);
            requestStartNanos = System.nanoTime();
            responseBytes = 0;
        }
        super.channelRead(ctx, msg);
    }

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) throws Exception {
        if (msg instanceof HttpResponse response) {
            HttpResponseStatus status = response.status();
            statusCode = status != null ? status.code() : 0;
        }

        if (msg instanceof HttpContent content) {
            responseBytes += content.content().readableBytes();
        }

        if (msg instanceof LastHttpContent) {
            long latencyMs = (System.nanoTime() - requestStartNanos) / 1_000_000;
            if (ACCESS_LOG.isInfoEnabled()) {
                // Structured JSON format for log aggregation (ELK, Loki, Datadog).
                // Manual StringBuilder avoids Jackson per-request allocation overhead.
                StringBuilder sb = new StringBuilder(128);
                sb.append("{\"method\":\"").append(method != null ? method : "-");
                sb.append("\",\"uri\":\"");
                jsonEscape(sb, uri);
                sb.append("\",\"status\":").append(statusCode);
                sb.append(",\"latencyMs\":").append(latencyMs);
                sb.append(",\"bytes\":").append(responseBytes);
                sb.append(",\"requestId\":\"");
                jsonEscape(sb, requestId);
                sb.append("\",\"protocol\":\"").append(protocol != null ? protocol : "-");
                if (tlsCipher != null) {
                    sb.append("\",\"tlsCipher\":\"").append(tlsCipher);
                }
                sb.append("\"}");
                ACCESS_LOG.info(sb.toString());
            }
            // Reset for next request on keep-alive connections
            method = null;
            uri = null;
            requestId = null;
            requestStartNanos = 0;
            statusCode = 0;
            responseBytes = 0;
        }
        super.write(ctx, msg, promise);
    }

    /**
     * Appends a JSON-safe representation of {@code value} to the StringBuilder.
     * Escapes characters that are special in JSON strings: backslash, double-quote,
     * and control characters (U+0000 through U+001F) per RFC 8259 Section 7.
     *
     * <p>This is intentionally minimal — it handles the characters that can appear
     * in URIs and request IDs without pulling in a JSON library on the hot path.</p>
     *
     * @param sb    the target StringBuilder
     * @param value the value to escape; if null, appends "-"
     */
    private static void jsonEscape(StringBuilder sb, String value) {
        if (value == null) {
            sb.append('-');
            return;
        }
        for (int i = 0, len = value.length(); i < len; i++) {
            char c = value.charAt(i);
            switch (c) {
                case '"'  -> sb.append("\\\"");
                case '\\' -> sb.append("\\\\");
                case '\n' -> sb.append("\\n");
                case '\r' -> sb.append("\\r");
                case '\t' -> sb.append("\\t");
                default -> {
                    if (c < 0x20) {
                        // Control characters: emit as unicode escape (e.g., backslash-u-00-XX)
                        sb.append("\\u00");
                        sb.append(Character.forDigit((c >> 4) & 0xF, 16));
                        sb.append(Character.forDigit(c & 0xF, 16));
                    } else {
                        sb.append(c);
                    }
                }
            }
        }
    }
}
