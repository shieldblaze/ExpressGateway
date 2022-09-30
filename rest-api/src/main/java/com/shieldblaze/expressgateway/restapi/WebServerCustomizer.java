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

import io.netty.handler.ssl.ClientAuth;
import io.netty.handler.ssl.OpenSsl;
import io.netty.handler.ssl.SslProvider;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.springframework.boot.web.embedded.netty.NettyReactiveWebServerFactory;
import org.springframework.boot.web.embedded.netty.NettyServerCustomizer;
import org.springframework.boot.web.server.WebServerFactoryCustomizer;
import org.springframework.context.annotation.Configuration;
import reactor.netty.http.Http2SslContextSpec;
import reactor.netty.http.HttpProtocol;
import reactor.netty.http.server.HttpServer;

import java.security.PrivateKey;
import java.security.cert.X509Certificate;
import java.util.List;

/**
 * This customizer is responsible for the configuration of
 * Netty web server, like Bind Address and Port, TLS, etc.
 */
@Configuration
public class WebServerCustomizer implements WebServerFactoryCustomizer<NettyReactiveWebServerFactory> {

    private static final Logger logger = LogManager.getLogger(WebServerCustomizer.class);

    @Override
    public void customize(NettyReactiveWebServerFactory factory) {
        /*try {
            InetAddress inetAddress = InetAddress.getByName(REST_API_IP_ADDRESS);
            factory.setAddress(inetAddress);
        } catch (Exception ex) {
            logger.error("Invalid REST-API Address, Shutting down...", ex);
            System.exit(1);
            return;
        }

        try {
            int port = Integer.parseInt(REST_API_PORT);
            factory.setPort(port);
        } catch (Exception ex) {
            logger.error("Invalid REST-API Port, Shutting down...", ex);
            System.exit(1);
            return;
        }

        try {
            Entry entry = DataStore.fetchPrivateKeyCertificateEntry(System.getProperty("datastore.password").toCharArray(),
                    System.getProperty("datastore.alias"));

            X509Certificate[] x509Certificates = new X509Certificate[entry.certificates().length];
            int i = 0;
            for (Certificate cert : entry.certificates()) {
                x509Certificates[i] = (X509Certificate) cert;
                i++;
            }

            factory.addServerCustomizers(new TlsCustomizer(entry.privateKey(), x509Certificates));
        } catch (Exception ex) {
            logger.error("Failed to load RestApi DataStore Entry");
        }*/
    }

    record TlsCustomizer(PrivateKey privateKey, X509Certificate[] x509Certificates) implements NettyServerCustomizer {

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
}
