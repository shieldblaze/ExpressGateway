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
package com.shieldblaze.expressgateway.servicediscovery.client;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.shieldblaze.expressgateway.common.ExpressGateway;
import com.shieldblaze.expressgateway.common.utils.StringUtil;
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;
import org.apache.zookeeper.common.X509Util;

import javax.net.ssl.KeyManager;
import javax.net.ssl.SSLContext;
import javax.net.ssl.TrustManager;
import java.io.IOException;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.security.SecureRandom;

public final class ServiceDiscoveryClient {

    private static final HttpClient HTTP_CLIENT;

    static {
        ExpressGateway.ServiceDiscovery serviceDiscovery = ExpressGateway.getInstance().serviceDiscovery();

        if (serviceDiscovery.URI().startsWith("https")) {
            try {
                if (serviceDiscovery.trustAllCerts()) {
                    SSLContext sslContext = SSLContext.getInstance("TLSv1.3");
                    sslContext.init(null, InsecureTrustManagerFactory.INSTANCE.getTrustManagers(), new SecureRandom());

                    HTTP_CLIENT = HttpClient.newBuilder()
                            .sslContext(sslContext)
                            .build();
                } else {
                    KeyManager[] keyManagers = null;
                    if (StringUtil.isNullOrEmpty(serviceDiscovery.keyStoreFile())) {
                        keyManagers = new KeyManager[]{X509Util.createKeyManager(
                                serviceDiscovery.keyStoreFile(),
                                String.valueOf(serviceDiscovery.keyStorePasswordAsChars()),
                                ""
                        )};
                    }

                    TrustManager[] trustManagers = {X509Util.createTrustManager(
                            serviceDiscovery.trustStoreFile(),
                            String.valueOf(serviceDiscovery.trustStorePasswordAsChars()),
                            "",
                            false,
                            false,
                            serviceDiscovery.hostnameVerification(),
                            serviceDiscovery.hostnameVerification(),
                            false
                    )};

                    SSLContext sslContext = SSLContext.getInstance("TLSv1.3");
                    sslContext.init(keyManagers, trustManagers, new SecureRandom());

                    HTTP_CLIENT = HttpClient.newBuilder()
                            .sslContext(sslContext)
                            .build();
                }
            } catch (Exception ex) {
                throw new RuntimeException(ex);
            }
        } else if (serviceDiscovery.URI().startsWith("http")) {
            HTTP_CLIENT = HttpClient.newHttpClient();
        } else {
            throw new IllegalArgumentException("Unsupported URI Protocol: " + serviceDiscovery.URI());
        }
    }

    /**
     * Register this service on service discovery
     *
     * @throws IOException          On error
     * @throws InterruptedException Thread interrupted while waiting for response
     */
    public static void register() throws IOException, InterruptedException {
        HttpRequest httpRequest = HttpRequest.newBuilder()
                .uri(URI.create(ExpressGateway.getInstance().serviceDiscovery().URI() + "/api/v1/service/register"))
                .PUT(HttpRequest.BodyPublishers.ofString(requestJson()))
                .setHeader("User-Agent", "ExpressGateway Service Discovery Client")
                .setHeader("Content-Type", "application/json")
                .build();

        HttpResponse<String> httpResponse = HTTP_CLIENT.send(httpRequest, HttpResponse.BodyHandlers.ofString());

        JsonObject response = JsonParser.parseString(httpResponse.body()).getAsJsonObject();
        if (!response.get("Success").getAsBoolean()) {
            throw new IllegalStateException("Registration failed, Response: " + response);
        }
    }

    /**
     * Deregister this service from service discovery
     *
     * @throws IOException          On error
     * @throws InterruptedException Thread interrupted while waiting for response
     */
    public static void deregister() throws IOException, InterruptedException {
        HttpRequest httpRequest = HttpRequest.newBuilder()
                .uri(URI.create(ExpressGateway.getInstance().serviceDiscovery().URI() + "/api/v1/service/deregister"))
                .method("DELETE", HttpRequest.BodyPublishers.ofString(requestJson()))
                .setHeader("User-Agent", "ExpressGateway Service Discovery Client")
                .setHeader("Content-Type", "application/json")
                .build();

        HttpResponse<String> httpResponse = HTTP_CLIENT.send(httpRequest, HttpResponse.BodyHandlers.ofString());

        JsonObject response = JsonParser.parseString(httpResponse.body()).getAsJsonObject();
        if (!response.get("Success").getAsBoolean()) {
            throw new IllegalStateException("Deregistration failed, Response: " + response);
        }
    }

    static String requestJson() {
        JsonObject node = new JsonObject();
        node.addProperty("ID", ExpressGateway.getInstance().ID());
        node.addProperty("IPAddress", ExpressGateway.getInstance().restApi().IPAddress());
        node.addProperty("Port", ExpressGateway.getInstance().restApi().port());
        node.addProperty("TLSEnabled", ExpressGateway.getInstance().restApi().enableTLS());
        return node.toString();
    }

    private ServiceDiscoveryClient() {
        // Prevent outside initialization
    }
}
