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

import com.shieldblaze.expressgateway.common.utils.NumberUtil;
import com.shieldblaze.expressgateway.common.utils.SelfSignedCertificate;
import com.shieldblaze.expressgateway.common.utils.SystemPropertyUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.springframework.boot.web.embedded.netty.NettyReactiveWebServerFactory;
import org.springframework.boot.web.server.Http2;
import org.springframework.boot.web.server.Shutdown;
import org.springframework.boot.web.server.WebServerFactoryCustomizer;
import org.springframework.context.annotation.Configuration;

import java.net.InetAddress;
import java.net.UnknownHostException;
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

        InetAddress inetAddress;
        int port;
        try {
            inetAddress = InetAddress.getByName(SystemPropertyUtil.getPropertyOrEnv("api-address"));
        } catch (Exception ex) {
            logger.error("Invalid REST-API Address, Falling back to 0.0.0.0", ex);
            try {
                inetAddress = InetAddress.getByName("0.0.0.0");
            } catch (UnknownHostException e) {
                logger.error("Invalid REST-API Address 0.0.0.0, This should never happen! Shutting down...");
                System.exit(1);
                return;
            }
        }

        try {
            port = NumberUtil.checkInRange(SystemPropertyUtil.getPropertyOrEnvInt("api-port", "9110"), 1, 65535, "REST-API Port");
        } catch (Exception ex) {
            logger.error("Invalid REST-API Port, Shutting down...", ex);
            System.exit(1);
            return;
        }

        factory.setAddress(inetAddress);
        factory.setPort(port);

        SelfSignedCertificate ssc = SelfSignedCertificate.generateNew(List.of("127.0.0.1"));
        factory.addServerCustomizers(new TlsCustomizer(ssc.keyPair().getPrivate(), ssc.x509Certificate()));
    }
}
