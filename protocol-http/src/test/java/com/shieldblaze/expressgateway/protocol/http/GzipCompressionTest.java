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
package com.shieldblaze.expressgateway.protocol.http;

import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.time.Duration;
import java.util.zip.GZIPInputStream;

import static org.junit.jupiter.api.Assertions.assertEquals;

class GzipCompressionTest {

    private static HttpClient httpClient;

    @BeforeAll
    static void setup() throws Exception {
        Common.initialize();
        httpClient = Common.httpClient;
    }

    @AfterAll
    static void shutdown() {
        Common.shutdown();
    }

    @Test
    void gzipOnlyTest() throws IOException, InterruptedException {
        HttpRequest httpRequest = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("https://localhost:9110"))
                .version(HttpClient.Version.HTTP_2)
                .timeout(Duration.ofSeconds(5))
                .setHeader("Accept-Encoding", "gzip")
                .build();

        HttpResponse<byte[]> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofByteArray());
        assertEquals(200, httpResponse.statusCode());
        assertEquals("gzip", httpResponse.headers().firstValue("Content-Encoding").get());

        GZIPInputStream gzipInputStream = new GZIPInputStream(new ByteArrayInputStream(httpResponse.body()));
        assertEquals("Meow", new String(gzipInputStream.readAllBytes()));
        gzipInputStream.close();
    }

    @Test
    void gzipAndDeflateTest() throws IOException, InterruptedException {
        HttpRequest httpRequest = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("https://localhost:9110"))
                .version(HttpClient.Version.HTTP_2)
                .timeout(Duration.ofSeconds(5))
                .setHeader("Accept-Encoding", "gzip, deflate")
                .build();

        HttpResponse<byte[]> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofByteArray());
        assertEquals(200, httpResponse.statusCode());
        assertEquals("gzip", httpResponse.headers().firstValue("Content-Encoding").get());

        GZIPInputStream gzipInputStream = new GZIPInputStream(new ByteArrayInputStream(httpResponse.body()));
        assertEquals("Meow", new String(gzipInputStream.readAllBytes()));
        gzipInputStream.close();
    }
}
