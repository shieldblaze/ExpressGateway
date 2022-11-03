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
package com.shieldblaze.expressgateway.servicediscovery.client;

import com.shieldblaze.expressgateway.common.ExpressGateway;
import com.shieldblaze.expressgateway.common.utils.StringUtil;
import org.apache.zookeeper.common.X509Util;

import javax.net.ssl.KeyManager;
import javax.net.ssl.SSLContext;
import javax.net.ssl.TrustManager;
import java.net.http.HttpClient;
import java.security.SecureRandom;

public final class Client {

    public static final HttpClient HTTP_CLIENT;

    static {
        ExpressGateway.ServiceDiscovery serviceDiscovery = ExpressGateway.getInstance().serviceDiscovery();

        if (serviceDiscovery.URI().startsWith("https")) {
            try {
                KeyManager[] keyManagers = null;
                if (StringUtil.isNullOrEmpty(serviceDiscovery.keyStoreFile())) {
                    keyManagers = new KeyManager[]{X509Util.createKeyManager(
                            serviceDiscovery.keyStoreFile(),
                            String.valueOf(serviceDiscovery.keyStorePasswordAsChars()),
                            ""
                    )};
                }

                TrustManager[] trustManagers = new TrustManager[]{X509Util.createTrustManager(
                        serviceDiscovery.trustStoreFile(),
                        String.valueOf(serviceDiscovery.trustStorePasswordAsChars()),
                        "",
                        false,
                        false,
                        serviceDiscovery.hostnameVerification(),
                        serviceDiscovery.hostnameVerification()
                )};

                SSLContext sslContext = SSLContext.getInstance("TLSv1.3");
                sslContext.init(keyManagers, trustManagers, new SecureRandom());

                HTTP_CLIENT = HttpClient.newBuilder()
                        .sslContext(sslContext)
                        .build();
            } catch (Exception ex) {
                throw new RuntimeException(ex);
            }
        } else if (serviceDiscovery.URI().startsWith("http")) {
            HTTP_CLIENT = HttpClient.newHttpClient();
        } else {
            throw new IllegalArgumentException("Unsupported URI Protocol: " + serviceDiscovery.URI());
        }
    }

    public static void register() {

    }
}
