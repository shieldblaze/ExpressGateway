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

import io.netty.buffer.Unpooled;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpVersion;
import org.junit.jupiter.api.Test;

import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class EmbeddedHttpServerTest {

    @Test
    void plainTextServer() throws Exception {
        try (EmbeddedHttpServer server = EmbeddedHttpServer.builder()
                .responseBody("Hello from test")
                .build()
                .start()) {

            assertTrue(server.port() > 0);
            assertFalse(server.isTls());
            assertTrue(server.baseUrl().startsWith("http://"));

            HttpClient client = HttpClient.newHttpClient();
            HttpRequest request = HttpRequest.newBuilder()
                    .uri(URI.create(server.baseUrl() + "/test"))
                    .GET()
                    .build();

            HttpResponse<String> response = client.send(request, HttpResponse.BodyHandlers.ofString());
            assertEquals(200, response.statusCode());
            assertEquals("Hello from test", response.body());
        }
    }

    @Test
    void customStatusCode() throws Exception {
        try (EmbeddedHttpServer server = EmbeddedHttpServer.builder()
                .responseBody("Not Found")
                .responseStatus(HttpResponseStatus.NOT_FOUND)
                .build()
                .start()) {

            HttpClient client = HttpClient.newHttpClient();
            HttpRequest request = HttpRequest.newBuilder()
                    .uri(URI.create(server.baseUrl() + "/missing"))
                    .GET()
                    .build();

            HttpResponse<String> response = client.send(request, HttpResponse.BodyHandlers.ofString());
            assertEquals(404, response.statusCode());
        }
    }

    @Test
    void customHandler() throws Exception {
        try (EmbeddedHttpServer server = EmbeddedHttpServer.builder()
                .handler(req -> {
                    String body = "Echo: " + req.uri();
                    byte[] bytes = body.getBytes(StandardCharsets.UTF_8);
                    DefaultFullHttpResponse resp = new DefaultFullHttpResponse(
                            HttpVersion.HTTP_1_1, HttpResponseStatus.OK, Unpooled.wrappedBuffer(bytes));
                    resp.headers().set("Content-Type", "text/plain");
                    resp.headers().setInt("Content-Length", bytes.length);
                    return resp;
                })
                .build()
                .start()) {

            HttpClient client = HttpClient.newHttpClient();
            HttpRequest request = HttpRequest.newBuilder()
                    .uri(URI.create(server.baseUrl() + "/my/path"))
                    .GET()
                    .build();

            HttpResponse<String> response = client.send(request, HttpResponse.BodyHandlers.ofString());
            assertEquals(200, response.statusCode());
            assertEquals("Echo: /my/path", response.body());
        }
    }

    @Test
    void tlsServer() throws Exception {
        try (SelfSignedCertificate cert = SelfSignedCertificate.create();
             EmbeddedHttpServer server = EmbeddedHttpServer.builder()
                     .tls(cert)
                     .responseBody("TLS OK")
                     .build()
                     .start()) {

            assertTrue(server.isTls());
            assertTrue(server.baseUrl().startsWith("https://"));

            // Build a TrustManager that trusts only the test certificate
            java.security.KeyStore trustStore = java.security.KeyStore.getInstance(java.security.KeyStore.getDefaultType());
            trustStore.load(null, null);
            trustStore.setCertificateEntry("test-cert", cert.certificate());
            javax.net.ssl.TrustManagerFactory tmf = javax.net.ssl.TrustManagerFactory.getInstance(
                    javax.net.ssl.TrustManagerFactory.getDefaultAlgorithm());
            tmf.init(trustStore);

            javax.net.ssl.SSLContext sslContext = javax.net.ssl.SSLContext.getInstance("TLS");
            sslContext.init(null, tmf.getTrustManagers(), new java.security.SecureRandom());

            HttpClient client = HttpClient.newBuilder().sslContext(sslContext).build();
            HttpRequest request = HttpRequest.newBuilder()
                    .uri(URI.create(server.baseUrl() + "/secure"))
                    .GET()
                    .build();

            HttpResponse<String> response = client.send(request, HttpResponse.BodyHandlers.ofString());
            assertEquals(200, response.statusCode());
            assertEquals("TLS OK", response.body());
        }
    }
}
