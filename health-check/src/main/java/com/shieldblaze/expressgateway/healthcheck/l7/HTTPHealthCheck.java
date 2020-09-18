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

import java.net.InetSocketAddress;
import java.net.URI;
import java.security.KeyManagementException;
import java.security.KeyStoreException;
import java.security.NoSuchAlgorithmException;

public final class HTTPHealthCheck extends HealthCheck {

    private final HttpClientBuilder httpClientBuilder;
    private final URI uri;

    public HTTPHealthCheck(URI uri, int timeout, boolean disableTLSValidation) throws KeyStoreException, NoSuchAlgorithmException,
            KeyManagementException {
        super(new InetSocketAddress(uri.getHost(), uri.getPort()), timeout);
        this.uri = uri;

        RequestConfig requestConfig = RequestConfig.custom()
                .setRedirectsEnabled(false)
                .setConnectionRequestTimeout(1000 * timeout)
                .setSocketTimeout(1000 * timeout)
                .setConnectTimeout(1000 *  timeout)
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
    public void check() {
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
            e.printStackTrace();
            markFailure();
        }
    }
}
