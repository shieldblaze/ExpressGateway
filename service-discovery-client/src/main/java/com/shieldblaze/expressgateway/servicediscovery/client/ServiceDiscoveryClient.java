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
import lombok.extern.slf4j.Slf4j;
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
import java.time.Duration;
import java.util.List;
import java.util.concurrent.locks.ReentrantLock;

/**
 * Enhanced service discovery client with retry, circuit breaker, local cache,
 * multi-server failover, and DNS-based fallback. All new features are opt-in
 * and backward-compatible: the existing static {@link #register()} and
 * {@link #deregister()} methods work identically to before.
 *
 * <p>New features are accessible via the {@link #builder()} API for creating
 * configured instances.</p>
 */
@Slf4j
public final class ServiceDiscoveryClient {

    // ---- Static legacy API (backward-compatible) ----

    /**
     * Lazy-initialized HTTP client. Replaced the static initializer block
     * so the class can be loaded even before ExpressGateway.getInstance()
     * is available (e.g., in builder-based usage).
     */
    private static final ReentrantLock INIT_LOCK = new ReentrantLock();
    private static volatile HttpClient legacyHttpClient;

    private static HttpClient getLegacyHttpClient() {
        if (legacyHttpClient == null) {
            INIT_LOCK.lock();
            try {
                if (legacyHttpClient == null) {
                    legacyHttpClient = buildHttpClient(ExpressGateway.getInstance().serviceDiscovery());
                }
            } finally {
                INIT_LOCK.unlock();
            }
        }
        return legacyHttpClient;
    }

    private static HttpClient buildHttpClient(ExpressGateway.ServiceDiscovery serviceDiscovery) {
        if (serviceDiscovery.URI().startsWith("https")) {
            try {
                if (serviceDiscovery.trustAllCerts()) {
                    SSLContext sslContext = SSLContext.getInstance("TLSv1.3");
                    sslContext.init(null, InsecureTrustManagerFactory.INSTANCE.getTrustManagers(), new SecureRandom());
                    return HttpClient.newBuilder().sslContext(sslContext).build();
                } else {
                    KeyManager[] keyManagers = null;
                    if (!StringUtil.isNullOrEmpty(serviceDiscovery.keyStoreFile())) {
                        keyManagers = new KeyManager[]{X509Util.createKeyManager(
                                serviceDiscovery.keyStoreFile(),
                                String.valueOf(serviceDiscovery.keyStorePasswordAsChars()),
                                "")};
                    }

                    TrustManager[] trustManagers = {X509Util.createTrustManager(
                            serviceDiscovery.trustStoreFile(),
                            String.valueOf(serviceDiscovery.trustStorePasswordAsChars()),
                            "", false, false,
                            serviceDiscovery.hostnameVerification(),
                            serviceDiscovery.hostnameVerification(),
                            false, false)};

                    SSLContext sslContext = SSLContext.getInstance("TLSv1.3");
                    sslContext.init(keyManagers, trustManagers, new SecureRandom());
                    return HttpClient.newBuilder().sslContext(sslContext).build();
                }
            } catch (Exception ex) {
                throw new RuntimeException(ex);
            }
        } else if (serviceDiscovery.URI().startsWith("http")) {
            return HttpClient.newHttpClient();
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

        HttpResponse<String> httpResponse = getLegacyHttpClient().send(httpRequest, HttpResponse.BodyHandlers.ofString());

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

        HttpResponse<String> httpResponse = getLegacyHttpClient().send(httpRequest, HttpResponse.BodyHandlers.ofString());

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

    // ---- Instance-based enhanced API ----

    private final HttpClient httpClient;
    private final DiscoveryServerPool serverPool;
    private final CircuitBreaker circuitBreaker;
    private final ServiceCache cache;
    private final RetryPolicy retryPolicy;
    private final DnsServiceDiscovery dnsFallback;

    private ServiceDiscoveryClient(HttpClient httpClient,
                                   DiscoveryServerPool serverPool,
                                   CircuitBreaker circuitBreaker,
                                   ServiceCache cache,
                                   RetryPolicy retryPolicy,
                                   DnsServiceDiscovery dnsFallback) {
        this.httpClient = httpClient;
        this.serverPool = serverPool;
        this.circuitBreaker = circuitBreaker;
        this.cache = cache;
        this.retryPolicy = retryPolicy;
        this.dnsFallback = dnsFallback;
    }

    /**
     * Register a service entry on the discovery server with retry, circuit breaker,
     * and failover.
     *
     * @param entry the service entry to register
     * @throws IOException if all retry attempts fail
     */
    public void registerService(ServiceEntry entry) throws IOException {
        String body = serviceEntryToJson(entry);
        executeWithResilience("/api/v1/service/register", "PUT", body);
        cache.put(entry.id(), entry);
        log.info("Registered service: {}", entry.id());
    }

    /**
     * Deregister a service entry from the discovery server.
     *
     * @param entry the service entry to deregister
     * @throws IOException if all retry attempts fail
     */
    public void deregisterService(ServiceEntry entry) throws IOException {
        String body = serviceEntryToJson(entry);
        executeWithResilience("/api/v1/service/deregister", "DELETE", body);
        cache.remove(entry.id());
        log.info("Deregistered service: {}", entry.id());
    }

    /**
     * Look up a service by ID. Checks cache first, then queries the discovery server,
     * and falls back to DNS if all else fails.
     *
     * @param serviceId the service ID to look up
     * @return the service entry, or null if not found
     */
    public ServiceEntry lookupService(String serviceId) {
        // Check cache first
        var cached = cache.get(serviceId);
        if (cached.isPresent() && cached.get().fresh()) {
            log.debug("Cache hit for service: {}", serviceId);
            return cached.get().entry();
        }

        // Try discovery server
        try {
            String responseBody = executeWithResilience(
                    "/api/v1/service/get?id=" + serviceId, "GET", null);
            JsonObject json = JsonParser.parseString(responseBody).getAsJsonObject();
            if (json.get("Success").getAsBoolean() && json.has("Instances")) {
                var instances = json.getAsJsonArray("Instances");
                if (!instances.isEmpty()) {
                    var instance = instances.get(0).getAsJsonObject();
                    var payload = instance.getAsJsonObject("payload");
                    ServiceEntry entry = ServiceEntry.of(
                            payload.get("ID").getAsString(),
                            payload.get("IPAddress").getAsString(),
                            payload.get("Port").getAsInt(),
                            payload.get("TLSEnabled").getAsBoolean());
                    cache.put(serviceId, entry);
                    return entry;
                }
            }
        } catch (Exception ex) {
            log.warn("Discovery lookup failed for {}, trying cache/DNS fallback", serviceId, ex);
        }

        // Return stale cache entry if available
        if (cached.isPresent()) {
            log.info("Returning stale cache entry for: {}", serviceId);
            return cached.get().entry();
        }

        // DNS fallback
        if (dnsFallback != null) {
            List<ServiceEntry> dnsEntries = dnsFallback.resolve();
            if (!dnsEntries.isEmpty()) {
                log.info("DNS fallback resolved {} entries", dnsEntries.size());
                cache.putAll(dnsEntries);
                return dnsEntries.getFirst();
            }
        }

        return null;
    }

    /**
     * Execute an HTTP request with retry, circuit breaker, and server failover.
     */
    String executeWithResilience(String path, String method, String body) throws IOException {
        circuitBreaker.allowRequest();

        IOException lastException = null;
        for (int attempt = 0; attempt < retryPolicy.maxAttempts(); attempt++) {
            if (attempt > 0) {
                long delayMs = retryPolicy.delayMillis(attempt);
                try {
                    Thread.sleep(delayMs);
                } catch (InterruptedException ie) {
                    Thread.currentThread().interrupt();
                    throw new IOException("Interrupted during retry backoff", ie);
                }
            }

            String serverUri = serverPool.selectServer();
            try {
                HttpRequest.Builder reqBuilder = HttpRequest.newBuilder()
                        .uri(URI.create(serverUri + path))
                        .setHeader("User-Agent", "ExpressGateway Service Discovery Client")
                        .setHeader("Content-Type", "application/json")
                        .timeout(Duration.ofSeconds(10));

                switch (method) {
                    case "PUT" -> reqBuilder.PUT(HttpRequest.BodyPublishers.ofString(body));
                    case "DELETE" -> reqBuilder.method("DELETE", HttpRequest.BodyPublishers.ofString(body));
                    default -> reqBuilder.GET();
                }

                HttpResponse<String> response = httpClient.send(reqBuilder.build(),
                        HttpResponse.BodyHandlers.ofString());

                if (response.statusCode() >= 200 && response.statusCode() < 300) {
                    serverPool.recordSuccess(serverUri);
                    circuitBreaker.recordSuccess();
                    return response.body();
                } else {
                    throw new IOException("HTTP " + response.statusCode() + ": " + response.body());
                }
            } catch (InterruptedException ie) {
                Thread.currentThread().interrupt();
                throw new IOException("Interrupted during HTTP call", ie);
            } catch (IOException ex) {
                lastException = ex;
                serverPool.recordFailure(serverUri);
                log.warn("Attempt {} to {} failed: {}", attempt + 1, serverUri + path, ex.getMessage());
            } catch (RuntimeException ex) {
                lastException = new IOException("Request failed: " + ex.getMessage(), ex);
                serverPool.recordFailure(serverUri);
                log.warn("Attempt {} to {} failed: {}", attempt + 1, serverUri + path, ex.getMessage());
            }
        }

        circuitBreaker.recordFailure();
        throw new IOException("All " + retryPolicy.maxAttempts() + " attempts failed", lastException);
    }

    private static String serviceEntryToJson(ServiceEntry entry) {
        JsonObject json = new JsonObject();
        json.addProperty("ID", entry.id());
        json.addProperty("IPAddress", entry.ipAddress());
        json.addProperty("Port", entry.port());
        json.addProperty("TLSEnabled", entry.tlsEnabled());
        return json.toString();
    }

    /**
     * Return the circuit breaker for monitoring/testing.
     */
    public CircuitBreaker circuitBreaker() {
        return circuitBreaker;
    }

    /**
     * Return the cache for monitoring/testing.
     */
    public ServiceCache cache() {
        return cache;
    }

    /**
     * Return the server pool for monitoring/testing.
     */
    public DiscoveryServerPool serverPool() {
        return serverPool;
    }

    /**
     * Create a new builder for a configured ServiceDiscoveryClient instance.
     */
    public static Builder builder() {
        return new Builder();
    }

    public static final class Builder {
        private HttpClient httpClient;
        private List<String> serverUris;
        private int unhealthyThreshold = 3;
        private int circuitBreakerFailureThreshold = 5;
        private Duration circuitBreakerResetTimeout = Duration.ofSeconds(30);
        private long cacheTtlMillis = 30_000;
        private RetryPolicy retryPolicy = RetryPolicy.DEFAULT;
        private String dnsFallbackName;
        private int dnsFallbackPort = 9110;

        private Builder() {
        }

        public Builder httpClient(HttpClient httpClient) {
            this.httpClient = httpClient;
            return this;
        }

        public Builder serverUris(List<String> serverUris) {
            this.serverUris = serverUris;
            return this;
        }

        public Builder serverUri(String serverUri) {
            this.serverUris = List.of(serverUri);
            return this;
        }

        public Builder unhealthyThreshold(int threshold) {
            this.unhealthyThreshold = threshold;
            return this;
        }

        public Builder circuitBreakerFailureThreshold(int threshold) {
            this.circuitBreakerFailureThreshold = threshold;
            return this;
        }

        public Builder circuitBreakerResetTimeout(Duration timeout) {
            this.circuitBreakerResetTimeout = timeout;
            return this;
        }

        public Builder cacheTtlMillis(long ttlMillis) {
            this.cacheTtlMillis = ttlMillis;
            return this;
        }

        public Builder retryPolicy(RetryPolicy retryPolicy) {
            this.retryPolicy = retryPolicy;
            return this;
        }

        public Builder dnsFallback(String dnsName, int defaultPort) {
            this.dnsFallbackName = dnsName;
            this.dnsFallbackPort = defaultPort;
            return this;
        }

        public ServiceDiscoveryClient build() {
            if (serverUris == null || serverUris.isEmpty()) {
                throw new IllegalStateException("At least one server URI must be configured");
            }
            HttpClient client = this.httpClient != null ? this.httpClient : HttpClient.newHttpClient();
            DiscoveryServerPool pool = new DiscoveryServerPool(serverUris, unhealthyThreshold);
            CircuitBreaker cb = new CircuitBreaker(circuitBreakerFailureThreshold, circuitBreakerResetTimeout);
            ServiceCache serviceCache = new ServiceCache(cacheTtlMillis);
            DnsServiceDiscovery dns = dnsFallbackName != null
                    ? new DnsServiceDiscovery(dnsFallbackName, dnsFallbackPort) : null;

            return new ServiceDiscoveryClient(client, pool, cb, serviceCache, retryPolicy, dns);
        }
    }
}
