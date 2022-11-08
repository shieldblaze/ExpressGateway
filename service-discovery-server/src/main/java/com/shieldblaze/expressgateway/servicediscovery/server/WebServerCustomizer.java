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
package com.shieldblaze.expressgateway.servicediscovery.server;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.shieldblaze.expressgateway.common.crypto.cryptostore.CryptoEntry;
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
import java.io.File;
import java.net.InetAddress;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.PrivateKey;
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
        String configFile = System.getProperty("config.file", "configuration.json");
        File file = new File(configFile);

        try {
            ObjectMapper objectMapper = new ObjectMapper();
            ServiceDiscoveryContext.setInstance(objectMapper.readValue(file, ServiceDiscoveryContext.class));
            logger.info("[CONFIGURATION] ServiceDiscovery: {}", ServiceDiscoveryContext.getInstance());

            factory.setAddress( InetAddress.getByName(ServiceDiscoveryContext.getInstance().IPAddress()));
            factory.setPort(ServiceDiscoveryContext.getInstance().port());

            if (ServiceDiscoveryContext.getInstance().enableTLS()) {
                logger.info("Loading Service Discovery Server PKCS12 File for TLS support");

                byte[] serverPkcs12Data = Files.readAllBytes(Path.of(ServiceDiscoveryContext.getInstance().PKCS12File()).toAbsolutePath());
                try (ByteArrayInputStream inputStream = new ByteArrayInputStream(serverPkcs12Data)) {
                    CryptoEntry cryptoEntry = fetchPrivateKeyCertificateEntry(inputStream, ServiceDiscoveryContext.getInstance().passwordAsChars(), null);
                    factory.addServerCustomizers(new TlsCustomizer(cryptoEntry.privateKey(), cryptoEntry.certificates()));
                }

                logger.info("Successfully initialized Service Discovery Server with TLS");
            } else {
                logger.info("Configuring Service Discovery Server without TLS");
                factory.addServerCustomizers(new NormalCustomizer());
            }
        } catch (Exception ex) {
            throw new IllegalArgumentException(ex);
        }
    }

    record TlsCustomizer(PrivateKey privateKey, X509Certificate[] x509Certificates) implements NettyServerCustomizer {

        @Override
        public HttpServer apply(HttpServer httpServer) {
            Http2SslContextSpec http2SslContextSpec = Http2SslContextSpec.forServer(privateKey, x509Certificates);
            http2SslContextSpec.configure(sslContextBuilder -> sslContextBuilder
                    .sslProvider(OpenSsl.isAvailable() ? SslProvider.OPENSSL : SslProvider.JDK)
                    .protocols("TLSv1.3")
                    .ciphers(List.of("TLS_AES_256_GCM_SHA384"))
                    .clientAuth(ServiceDiscoveryContext.getInstance().mTLS())
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
