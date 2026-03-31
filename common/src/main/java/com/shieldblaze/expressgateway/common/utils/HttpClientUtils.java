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
package com.shieldblaze.expressgateway.common.utils;

import io.netty.handler.ssl.util.InsecureTrustManagerFactory;

import javax.net.ssl.SSLContext;
import javax.net.ssl.TrustManagerFactory;
import java.net.http.HttpClient;
import java.security.KeyStore;
import java.security.SecureRandom;

public final class HttpClientUtils {

    /**
     * {@link HttpClient} that uses the system default trust store for TLS certificate validation.
     */
    public static final HttpClient CLIENT;

    /**
     * {@link HttpClient} that trusts all TLS certificates (including self-signed).
     * <b>Use only for testing or explicitly opted-in insecure configurations.</b>
     */
    public static final HttpClient INSECURE_CLIENT;

    static {
        try {
            // Secure client using system default trust store
            TrustManagerFactory tmf = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
            tmf.init((KeyStore) null);

            SSLContext secureSslContext = SSLContext.getInstance("TLSv1.3");
            secureSslContext.init(null, tmf.getTrustManagers(), new SecureRandom());

            CLIENT = HttpClient.newBuilder()
                    .sslContext(secureSslContext)
                    .build();

            // Insecure client for testing / opt-in insecure mode
            SSLContext insecureSslContext = SSLContext.getInstance("TLSv1.3");
            insecureSslContext.init(null, InsecureTrustManagerFactory.INSTANCE.getTrustManagers(), new SecureRandom());

            INSECURE_CLIENT = HttpClient.newBuilder()
                    .sslContext(insecureSslContext)
                    .build();
        } catch (Exception ex) {
            throw new RuntimeException(ex);
        }
    }

    private HttpClientUtils() {
        // Prevent outside initialization
    }
}
