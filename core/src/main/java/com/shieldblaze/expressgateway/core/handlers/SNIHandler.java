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

import com.shieldblaze.expressgateway.configuration.tls.CertificateKeyPair;
import com.shieldblaze.expressgateway.configuration.tls.TlsConfiguration;
import io.netty.channel.ChannelHandlerContext;
import io.netty.handler.codec.DecoderException;
import io.netty.handler.ssl.AbstractSniHandler;
import io.netty.handler.ssl.ReferenceCountedOpenSslEngine;
import io.netty.handler.ssl.SslHandler;
import io.netty.util.AsyncMapping;
import io.netty.util.AttributeKey;
import io.netty.util.ReferenceCountUtil;
import io.netty.util.concurrent.Future;
import lombok.extern.log4j.Log4j2;

import javax.net.ssl.SSLEngine;

/**
 * {@link SNIHandler} handles TLS Server Name Indication (SNI) and serves the correct
 * {@link CertificateKeyPair} as requested in SNI.
 *
 * <p>After TLS handshake completes, the negotiated ALPN protocol (h2, http/1.1) is
 * stored in the channel attribute {@link #NEGOTIATED_PROTOCOL} for downstream handlers
 * to use for protocol routing decisions.</p>
 */
@Log4j2
public final class SNIHandler extends AbstractSniHandler<CertificateKeyPair> {

    /**
     * Channel attribute key for the ALPN-negotiated protocol (e.g., "h2", "http/1.1").
     * Set after TLS handshake completes. Downstream handlers can use this for
     * protocol-specific pipeline construction.
     */
    public static final AttributeKey<String> NEGOTIATED_PROTOCOL =
            AttributeKey.valueOf("TLS_NEGOTIATED_PROTOCOL");

    /**
     * Channel attribute key for the SNI hostname extracted during TLS handshake.
     * Used for hostname-based routing decisions.
     */
    public static final AttributeKey<String> SNI_HOSTNAME =
            AttributeKey.valueOf("TLS_SNI_HOSTNAME");

    private final AsyncMapping<String, CertificateKeyPair> promise;

    public SNIHandler(TlsConfiguration tlsConfiguration) {
        promise = (input, promise) -> {
            try {
                return promise.setSuccess(tlsConfiguration.mapping(input));
            } catch (Exception ex) {
                return promise.setFailure(ex);
            }
        };
    }

    @Override
    protected Future<CertificateKeyPair> lookup(ChannelHandlerContext ctx, String hostname) {
        return promise.map(hostname, ctx.executor().newPromise());
    }

    @Override
    protected void onLookupComplete(ChannelHandlerContext ctx, String hostname, Future<CertificateKeyPair> future) {
        if (!future.isSuccess()) {
            final Throwable cause = future.cause();
            log.warn("SNI lookup failed for hostname: {}", hostname, cause);
            if (cause instanceof Error error) {
                throw error;
            }
            throw new DecoderException("Failed to get the CertificateKeyPair for: " + hostname, cause);
        }

        // Store the SNI hostname for downstream routing
        ctx.channel().attr(SNI_HOSTNAME).set(hostname);

        CertificateKeyPair certificateKeyPair = future.getNow();
        replaceHandler(ctx, certificateKeyPair);
    }

    private void replaceHandler(ChannelHandlerContext ctx, CertificateKeyPair certificateKeyPair) {
        SslHandler sslHandler = null;
        try {
            // Use newEngine() directly to avoid creating an intermediate SslHandler
            // that would be discarded. With the OpenSSL provider, discarding the
            // intermediate SslHandler leaks native memory (ReferenceCountedOpenSslEngine).
            SSLEngine engine = certificateKeyPair.sslContext().newEngine(ctx.alloc());
            SslHandler handler = new TLSHandler(engine);
            sslHandler = handler;
            // RES-01: Explicitly set TLS handshake timeout to defend against slow-TLS attacks.
            // Netty's default is 10s, but we set it explicitly for defense-in-depth.
            handler.setHandshakeTimeoutMillis(10_000);

            try {
                if (engine instanceof ReferenceCountedOpenSslEngine openSslEngine && certificateKeyPair.useOCSPStapling()) {
                    openSslEngine.setOcspResponse(certificateKeyPair.ocspStaplingData());
                }
            } catch (Exception ex) {
                ctx.fireExceptionCaught(ex);
            }

            // Capture ALPN-negotiated protocol after TLS handshake completes.
            // This enables downstream handlers to route based on the negotiated
            // application protocol (h2, http/1.1).
            handler.handshakeFuture().addListener(f -> {
                if (f.isSuccess()) {
                    String protocol = handler.applicationProtocol();
                    if (protocol != null) {
                        ctx.channel().attr(NEGOTIATED_PROTOCOL).set(protocol);
                        log.debug("ALPN negotiated protocol: {}", protocol);
                    }
                }
            });

            ctx.pipeline().replace(this, "TLSHandler", handler);
            sslHandler = null;
        } finally {
            // Since the SslHandler was not inserted into the pipeline the ownership of the SSLEngine was not
            // transferred to the SslHandler.
            // See https://github.com/netty/netty/issues/5678
            if (sslHandler != null) {
                ReferenceCountUtil.safeRelease(sslHandler.engine());
            }
        }
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        ctx.fireExceptionCaught(cause);
    }

    private static final class TLSHandler extends SslHandler {
        private TLSHandler(SSLEngine engine) {
            super(engine);
        }

        @Override
        public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
            ctx.fireExceptionCaught(cause);
        }
    }
}
