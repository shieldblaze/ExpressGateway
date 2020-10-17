/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
import org.apache.http.client.config.RequestConfig;
import org.apache.http.client.methods.CloseableHttpResponse;
import org.apache.http.client.methods.HttpGet;
import org.apache.http.conn.ssl.NoopHostnameVerifier;
import org.apache.http.conn.ssl.SSLConnectionSocketFactory;
import org.apache.http.conn.ssl.TrustSelfSignedStrategy;
import org.apache.http.impl.client.CloseableHttpClient;
import org.apache.http.impl.client.HttpClientBuilder;
import org.apache.http.ssl.SSLContextBuilder;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;
import java.net.URI;
import java.security.KeyManagementException;
import java.security.KeyStoreException;
import java.security.NoSuchAlgorithmException;
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

    private final HttpClientBuilder httpClientBuilder;
    private final URI uri;

    public HTTPHealthCheck(URI uri, Duration timeout, boolean disableTLSValidation) throws KeyStoreException, NoSuchAlgorithmException,
            KeyManagementException {
        super(new InetSocketAddress(uri.getHost(), uri.getPort()), timeout);
        this.uri = uri;

        RequestConfig requestConfig = RequestConfig.custom()
                .setRedirectsEnabled(false)
                .setConnectionRequestTimeout(super.timeout)
                .setSocketTimeout(super.timeout)
                .setConnectTimeout(super.timeout)
                .setAuthenticationEnabled(false)
                .setRedirectsEnabled(false)
                .setCircularRedirectsAllowed(false)
                .build();

        this.httpClientBuilder = HttpClientBuilder.create()
                .disableAuthCaching()
                .disableContentCompression()
                .disableAutomaticRetries()
                .disableConnectionState()
                .disableCookieManagement()
                .disableDefaultUserAgent()
                .setDefaultRequestConfig(requestConfig)
                .disableRedirectHandling();

        if (disableTLSValidation) {
            SSLContextBuilder builder = new SSLContextBuilder();
            builder.loadTrustMaterial(null, new TrustSelfSignedStrategy());
            SSLConnectionSocketFactory socketFactory = new SSLConnectionSocketFactory(builder.build());
            httpClientBuilder.setSSLSocketFactory(socketFactory);
            httpClientBuilder.setSSLHostnameVerifier(new NoopHostnameVerifier());
        }
    }

    @Override
    public void run() {
        try {
            try (CloseableHttpClient httpClient = httpClientBuilder.build()) {
                HttpGet httpGet = new HttpGet(uri);
                httpGet.addHeader("User-Agent", "ShieldBlaze ExpressGateway HealthCheck Agent");
                httpGet.addHeader("Connection", "Close");

                try (CloseableHttpResponse httpResponse = httpClient.execute(httpGet)) {
                    int status = httpResponse.getStatusLine().getStatusCode();
                    if (status >= 200 && status <= 299) {
                        markSuccess();
                    } else {
                        markFailure();
                    }
                }
            }
        } catch (Exception e) {
            logger.debug("Health Check Failure For Address: " + socketAddress, e);
            markFailure();
        }
    }
}
