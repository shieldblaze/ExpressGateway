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
package com.shieldblaze.expressgateway.testing;

import org.junit.jupiter.api.Test;

import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertThrows;

class HttpResponseAssertTest {

    @Test
    void assertAgainstLiveResponse() throws Exception {
        try (EmbeddedHttpServer server = EmbeddedHttpServer.builder()
                .responseBody("{\"Success\": true}")
                .build()
                .start()) {

            HttpClient client = HttpClient.newHttpClient();
            HttpRequest request = HttpRequest.newBuilder()
                    .uri(URI.create(server.baseUrl() + "/test"))
                    .GET()
                    .build();

            HttpResponse<String> response = client.send(request, HttpResponse.BodyHandlers.ofString());

            assertDoesNotThrow(() -> HttpResponseAssert.assertThat(response)
                    .hasStatusCode(200)
                    .isSuccessful()
                    .hasBody()
                    .bodyContains("Success")
                    .bodyContains("true"));
        }
    }

    @Test
    void wrongStatusCodeFails() throws Exception {
        try (EmbeddedHttpServer server = EmbeddedHttpServer.builder()
                .responseBody("OK")
                .build()
                .start()) {

            HttpClient client = HttpClient.newHttpClient();
            HttpRequest request = HttpRequest.newBuilder()
                    .uri(URI.create(server.baseUrl() + "/test"))
                    .GET()
                    .build();

            HttpResponse<String> response = client.send(request, HttpResponse.BodyHandlers.ofString());

            assertThrows(AssertionError.class,
                    () -> HttpResponseAssert.assertThat(response).hasStatusCode(404));
        }
    }

    @Test
    void missingHeaderFails() throws Exception {
        try (EmbeddedHttpServer server = EmbeddedHttpServer.builder()
                .responseBody("OK")
                .build()
                .start()) {

            HttpClient client = HttpClient.newHttpClient();
            HttpRequest request = HttpRequest.newBuilder()
                    .uri(URI.create(server.baseUrl() + "/test"))
                    .GET()
                    .build();

            HttpResponse<String> response = client.send(request, HttpResponse.BodyHandlers.ofString());

            assertThrows(AssertionError.class,
                    () -> HttpResponseAssert.assertThat(response).hasHeader("X-Custom-Missing"));
        }
    }
}
