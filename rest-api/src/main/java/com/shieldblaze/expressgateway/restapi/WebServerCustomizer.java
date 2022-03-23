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

import com.shieldblaze.expressgateway.common.datastore.DataStore;
import com.shieldblaze.expressgateway.common.utils.SelfSignedCertificate;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.springframework.boot.web.embedded.netty.NettyReactiveWebServerFactory;
import org.springframework.boot.web.server.WebServerFactoryCustomizer;
import org.springframework.context.annotation.Configuration;

import java.net.InetAddress;
import java.net.InetSocketAddress;
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
        try {
            InetAddress inetAddress = InetAddress.getByName(System.getProperty("restapi-address", "127.0.0.1"));
            factory.setAddress(inetAddress);
        } catch (Exception ex) {
            logger.error("Invalid REST-API Address, Shutting down...", ex);
            System.exit(1);
            return;
        }

        try {
            int port = new InetSocketAddress(Integer.parseInt(System.getProperty("restapi-port", "9110"))).getPort();
            factory.setPort(port);
        } catch (Exception ex) {
            logger.error("Invalid REST-API Port, Shutting down...", ex);
            System.exit(1);
            return;
        }

        // todo: add datastore support
        try {
            DataStore.INSTANCE.get("".toCharArray(), "RestApi");
        } catch (Exception ex) {
            logger.error("Failed to load RestApi DataStore Entry");
        }

        SelfSignedCertificate ssc = SelfSignedCertificate.generateNew(List.of("127.0.0.1"));
        factory.addServerCustomizers(new TlsCustomizer(ssc.keyPair().getPrivate(), new X509Certificate[]{ssc.x509Certificate()}));
    }
}
