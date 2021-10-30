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
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import javax.net.ssl.SSLContext;
import java.net.InetSocketAddress;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.security.KeyManagementException;
import java.security.NoSuchAlgorithmException;
import java.security.SecureRandom;
import java.time.Duration;

/**
 * <p> HTTP based {@link HealthCheck} </p>
 * <p> How it works:
 * <ol>
 *     <li> It starts a HTTP client and connects to remote host. </li>
 *     <li> If connection is successful and host replies with HTTP Response code between 200-299,
 *     it'll pass the Health Check and close the connection. </li>
 *     <li> If connection is not successful or host does not replies with HTTP Response code between 200-299,
 *     it'll fail the Health Check. </li>
 * </ol>
 * </p>
 */
public final class HTTPHealthCheck extends HealthCheck {

    private static final Logger logger = LogManager.getLogger(HTTPHealthCheck.class);

    private final HttpClient httpClient;
    private final URI uri;

    public HTTPHealthCheck(URI uri, Duration timeout, int samples) {
        super(new InetSocketAddress(uri.getHost(), uri.getPort()), timeout, samples);
        this.uri = uri;

        final SSLContext sslContext;
        try {
            sslContext = SSLContext.getInstance("TLSv1.3");
            sslContext.init(null, InsecureTrustManagerFactory.INSTANCE.getTrustManagers(), new SecureRandom());
        } catch (NoSuchAlgorithmException | KeyManagementException e) {
            // This should never happen
            Error error = new Error(e);
            logger.fatal(error);
            throw error;
        }

        httpClient = HttpClient.newBuilder()
                .followRedirects(HttpClient.Redirect.ALWAYS)
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
