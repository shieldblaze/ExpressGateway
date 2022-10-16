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

import com.shieldblaze.expressgateway.common.ExpressGateway;
import com.shieldblaze.expressgateway.common.crypto.cryptostore.CryptoEntry;
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

import java.io.ByteArrayInputStream;
import java.net.InetAddress;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.KeyStoreException;
import java.security.NoSuchAlgorithmException;
import java.security.PrivateKey;
import java.security.UnrecoverableKeyException;
import java.security.cert.CertificateException;
import java.security.cert.X509Certificate;
import java.util.List;

import static com.shieldblaze.expressgateway.common.crypto.cryptostore.CryptoStore.fetchPrivateKeyCertificateEntry;

/**
 * This customizer is responsible for the configuration of
 * Netty web server, like Bind Address and Port, TLS, etc.
 */
@Configuration
public class WebServerCustomizer implements WebServerFactoryCustomizer<NettyReactiveWebServerFactory> {

    private static final Logger logger = LogManager.getLogger(WebServerCustomizer.class);

    @Override
    public void customize(NettyReactiveWebServerFactory factory) {
        try {
            InetAddress inetAddress = InetAddress.getByName(ExpressGateway.getInstance().restApi().IPAddress());
            factory.setAddress(inetAddress);
        } catch (Exception ex) {
            logger.error("Error while configuring Rest-API server address, Shutting down...", ex);
            System.exit(1);
            return;
        }

        try {
            int port = ExpressGateway.getInstance().restApi().port();
            factory.setPort(port);
        } catch (Exception ex) {
            logger.error("Error while configuring Rest-API server port, Shutting down...", ex);
            System.exit(1);
            return;
        }

        try {
            if (ExpressGateway.getInstance().restApi().enableTLS()) {
                logger.info("Loading Rest-API PKCS12 File for TLS support");

                byte[] restApiPkcs12Data = Files.readAllBytes(Path.of(ExpressGateway.getInstance().restApi().PKCS12File()).toAbsolutePath());
                try (ByteArrayInputStream inputStream = new ByteArrayInputStream(restApiPkcs12Data)) {
                    CryptoEntry cryptoEntry = fetchPrivateKeyCertificateEntry(inputStream, ExpressGateway.getInstance().restApi().passwordAsChars(), "rest-api");
                    factory.addServerCustomizers(new TlsCustomizer(cryptoEntry.privateKey(), cryptoEntry.certificates()));
                }

                logger.info("Successfully initialized Rest-API Server with TLS");
            } else {
                logger.info("Configuring Rest-API Netty Server without TLS");
                factory.addServerCustomizers(new NormalCustomizer());
            }
        } catch (Exception ex) {
            logger.error("Failed to load initialize TLS configuration for REST-API server", ex);
            System.exit(1);
        }
    }

    /**
     * This customizer uses TLS
     *
     * @param privateKey       {@link PrivateKey} to use for TLS
     * @param x509Certificates {@link X509Certificate}s to use for TLS
     */
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
                    .protocol(HttpProtocol.H2, HttpProtocol.HTTP11);
        }
    }

    /**
     * This customizer doesn't use TLS
     */
    record NormalCustomizer() implements NettyServerCustomizer {

        @Override
        public HttpServer apply(HttpServer httpServer) {
            return httpServer.noSSL().protocol(HttpProtocol.H2C, HttpProtocol.HTTP11);
        }
    }
}
