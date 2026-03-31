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
package com.shieldblaze.expressgateway.healthcheck.l7;

import com.shieldblaze.expressgateway.healthcheck.HealthCheck;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import javax.net.ssl.SSLContext;
import javax.net.ssl.TrustManagerFactory;
import java.net.InetSocketAddress;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.security.KeyManagementException;
import java.security.KeyStore;
import java.security.NoSuchAlgorithmException;
import java.security.SecureRandom;
import java.time.Duration;

/**
 * HTTP based {@link HealthCheck}.
 *
 * <p>Performs an HTTP GET and considers 2xx responses healthy.
 * Uses secure TLS with system trust store by default.
 * Supports configurable request timeout enforcement.</p>
 */
public final class HTTPHealthCheck extends HealthCheck {

    private static final Logger logger = LogManager.getLogger(HTTPHealthCheck.class);

    private final HttpClient httpClient;
    private final URI uri;
    private final Duration requestTimeout;

    /**
     * Create with system default trust store (secure TLS verification).
     */
    public HTTPHealthCheck(URI uri, Duration timeout, int samples) {
        this(uri, timeout, samples, false);
    }

    /**
     * Create with optional insecure TLS.
     */
    public HTTPHealthCheck(URI uri, Duration timeout, int samples, boolean insecureTls) {
        super(new InetSocketAddress(uri.getHost(), uri.getPort()), timeout, samples);
        this.uri = uri;
        this.requestTimeout = timeout;

        final SSLContext sslContext;
        try {
            sslContext = SSLContext.getInstance("TLSv1.3");

            if (insecureTls) {
                logger.warn("Health check for {} using INSECURE TLS", uri);
                sslContext.init(null,
                        io.netty.handler.ssl.util.InsecureTrustManagerFactory.INSTANCE.getTrustManagers(),
                        new SecureRandom());
            } else {
                TrustManagerFactory tmf = TrustManagerFactory.getInstance(
                        TrustManagerFactory.getDefaultAlgorithm());
                tmf.init((KeyStore) null);
                sslContext.init(null, tmf.getTrustManagers(), new SecureRandom());
            }
        } catch (NoSuchAlgorithmException | KeyManagementException | java.security.KeyStoreException e) {
            Error error = new Error(e);
            logger.fatal(error);
            throw error;
        }

        httpClient = HttpClient.newBuilder()
                .followRedirects(HttpClient.Redirect.NEVER)
                .sslContext(sslContext)
                .connectTimeout(timeout)
                .build();
    }

    @Override
    public void run() {
        try {
            HttpRequest request = HttpRequest.newBuilder()
                    .GET()
                    .setHeader("User-Agent", "ExpressGateway HealthCheck Agent")
                    .uri(uri)
                    .timeout(requestTimeout)
                    .build();

            HttpResponse<Void> response = httpClient.send(request, HttpResponse.BodyHandlers.discarding());

            if (response.statusCode() >= 200 && response.statusCode() <= 299) {
                markSuccess();
            } else {
                markFailure();
            }
        } catch (Exception e) {
            logger.debug("Health Check Failure For Address: " + socketAddress, e);
            markFailure();
        }
    }
}
