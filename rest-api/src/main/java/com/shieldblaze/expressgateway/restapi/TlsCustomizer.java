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

package com.shieldblaze.expressgateway.restapi;

import com.shieldblaze.expressgateway.common.annotation.NonNull;
import io.netty.handler.ssl.ClientAuth;
import io.netty.handler.ssl.OpenSsl;
import io.netty.handler.ssl.SslProvider;
import org.springframework.boot.web.embedded.netty.NettyServerCustomizer;
import reactor.netty.http.Http2SslContextSpec;
import reactor.netty.http.HttpProtocol;
import reactor.netty.http.server.HttpServer;

import java.security.PrivateKey;
import java.security.cert.X509Certificate;
import java.util.List;

record TlsCustomizer(PrivateKey privateKey, X509Certificate[] x509Certificates) implements NettyServerCustomizer {

    @NonNull
    TlsCustomizer {
    }

    @Override
    public HttpServer apply(HttpServer httpServer) {
        Http2SslContextSpec http2SslContextSpec = Http2SslContextSpec.forServer(privateKey, x509Certificates);
        http2SslContextSpec.configure(sslContextBuilder -> sslContextBuilder
                .sslProvider(OpenSsl.isAvailable() ? SslProvider.OPENSSL : SslProvider.JDK)
                .protocols("TLSv1.3")
                .ciphers(List.of("TLS_AES_256_GCM_SHA384"))
                .clientAuth(ClientAuth.NONE)
        );

        return httpServer.secure(sslContextSpec -> sslContextSpec.sslContext(http2SslContextSpec))
                .protocol(HttpProtocol.H2);
    }
}
