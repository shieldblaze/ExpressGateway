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

import org.bouncycastle.jce.provider.BouncyCastleProvider;
import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;
import org.springframework.context.ConfigurableApplicationContext;

import java.security.Security;

@SpringBootApplication
public class RestAPI {

    private static ConfigurableApplicationContext ctx;

    static {
        // Initialize BouncyCastle first because we need it for TLS configuration
        Security.addProvider(new BouncyCastleProvider());
    }

    /**
     * Start the REST API Server using self-signed certificate.
     */
    public static void start() {
        if (ctx == null) {
            ctx = SpringApplication.run(RestAPI.class);
        } else {
            stop();
            ctx.start();
        }
    }

    /**
     * Shutdown REST-API Server
     */
    public static void stop() {
        if (ctx != null) {
            ctx.stop();
        }
    }
}
