/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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

import com.shieldblaze.expressgateway.protocol.http.Common;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

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
class Http2TLSBackendAndNonTLSClientTest {

    private static HttpClient httpClient;

    @BeforeEach
    void setup() throws Exception {
        Common.initialize(true, false, true);
        httpClient = Common.httpClient;
    }

    @AfterEach
    void shutdown() {
        Common.shutdown();
    }

    @Test
    void http11Test() throws Exception {
        HttpRequest httpRequest = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("http://localhost:9110"))
                .version(HttpClient.Version.HTTP_1_1)
                .timeout(Duration.ofSeconds(5))
                .build();

        HttpResponse<String> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(200, httpResponse.statusCode());
        assertEquals("Meow", httpResponse.body());
    }
}
