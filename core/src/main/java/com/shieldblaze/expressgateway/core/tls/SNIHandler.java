/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.core.tls;

import com.shieldblaze.expressgateway.configuration.tls.CertificateKeyPair;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfiguration;
import io.netty.channel.ChannelHandlerContext;
import io.netty.handler.codec.DecoderException;
import io.netty.handler.ssl.AbstractSniHandler;
import io.netty.handler.ssl.ReferenceCountedOpenSslEngine;
import io.netty.handler.ssl.SslHandler;
import io.netty.util.AsyncMapping;
import io.netty.util.ReferenceCountUtil;
import io.netty.util.concurrent.Future;

import javax.net.ssl.SSLEngine;

/**
 * {@link SNIHandler} TLS Server Name Indication (SNI) and serve the correct
 * {@link CertificateKeyPair} as requested in SNI.
 */
public final class SNIHandler extends AbstractSniHandler<CertificateKeyPair> {

    private final AsyncMapping<String, CertificateKeyPair> promise;

    public SNIHandler(TLSConfiguration tlsConfiguration) {
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
            if (cause instanceof Error) {
                throw (Error) cause;
            }
            throw new DecoderException("Failed to get the CertificateKeyPair for " + hostname, cause);
        }

        CertificateKeyPair certificateKeyPair = future.getNow();
        replaceHandler(ctx, certificateKeyPair);
    }

    protected void replaceHandler(ChannelHandlerContext ctx, CertificateKeyPair certificateKeyPair) {
        SslHandler sslHandler = null;
        try {
            sslHandler = new TLSHandler(certificateKeyPair.sslContext().newHandler(ctx.alloc()).engine());

            try {
                if (sslHandler.engine() instanceof ReferenceCountedOpenSslEngine && certificateKeyPair.useOCSP()) {
                    ((ReferenceCountedOpenSslEngine) sslHandler.engine()).setOcspResponse(certificateKeyPair.ocspStaplingData());
                }
            } catch (Exception ex) {
                ctx.fireExceptionCaught(ex);
            }

            ctx.pipeline().replace(this, "TLSHandler", sslHandler);
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
