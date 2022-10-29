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
package com.shieldblaze.expressgateway.protocol.http.adapter.mix;

import com.shieldblaze.expressgateway.protocol.http.TestableHttpLoadBalancer;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.io.IOException;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.time.Duration;

import static org.junit.jupiter.api.Assertions.assertEquals;

/**
 * Backend: TLS
 * TLS Client: TLS
 * TLS Server: Non-TLS
 */
class Http11BackendAndNonTLSClientTest {

    private static TestableHttpLoadBalancer testableHttpLoadBalancer;

    @BeforeAll
    static void setup() throws Exception {
        testableHttpLoadBalancer = TestableHttpLoadBalancer.Builder.newBuilder()
                .withTlsBackendEnabled(true)
                .withTlsClientEnabled(true)
                .withTlsServerEnabled(false)
                .build();

        testableHttpLoadBalancer.start();
    }

    @AfterAll
    static void shutdown() {
        testableHttpLoadBalancer.close();
    }

    @Test
    void http11ClientTest() throws IOException, InterruptedException {
        HttpRequest httpRequest = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("http://localhost:9110"))
                .version(HttpClient.Version.HTTP_1_1)
                .timeout(Duration.ofSeconds(5))
                .build();

        HttpResponse<String> httpResponse = TestableHttpLoadBalancer.httpClient().send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(200, httpResponse.statusCode());
        assertEquals("Meow", httpResponse.body());
    }
}
