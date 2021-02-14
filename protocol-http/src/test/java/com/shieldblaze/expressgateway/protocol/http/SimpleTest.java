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
package com.shieldblaze.expressgateway.protocol.http;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.CertificateKeyPair;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopEvent;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;
import io.netty.handler.ssl.util.SelfSignedCertificate;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import javax.net.ssl.SSLContext;
import java.net.InetSocketAddress;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.security.SecureRandom;
import java.time.Duration;
import java.util.Collections;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

class SimpleTest {

    static TLSConfiguration forServer;
    static TLSConfiguration forClient;
    static HttpClient httpClient;

    @BeforeAll
    static void initialize() throws Exception {

        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("localhost", "EC", 256);

        CertificateKeyPair certificateKeyPair = new CertificateKeyPair(Collections.singletonList(selfSignedCertificate.cert()), selfSignedCertificate.key());

        forServer = TLSConfiguration.DEFAULT_SERVER;
        forServer.addMapping("localhost", certificateKeyPair);

        forClient = TLSConfiguration.DEFAULT_CLIENT;
        forClient.acceptAllCerts(true);

        SSLContext sslContext = SSLContext.getInstance("TLSv1.3");
        sslContext.init(null, InsecureTrustManagerFactory.INSTANCE.getTrustManagers(), new SecureRandom());

        httpClient = HttpClient.newBuilder()
                .sslContext(sslContext)
                .build();
    }

    @Test
    void http2BackendWithTLSClient() throws Exception {
        HTTPServer httpServer = new HTTPServer(10000, true);
        httpServer.start();
        Thread.sleep(500L);

        Cluster cluster = new ClusterPool(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE));

        HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withCoreConfiguration(CoreConfiguration.DEFAULT)
                .withHTTPConfiguration(HTTPConfiguration.DEFAULT)
                .withTLSForClient(forClient)
                .withTLSForServer(forServer)
                .withBindAddress(new InetSocketAddress("localhost", 20000))
                .withHTTPInitializer(new DefaultHTTPServerInitializer())
                .withL4FrontListener(new TCPListener())
                .build();

        httpLoadBalancer.mapCluster("localhost:20000", cluster);
        new Node(cluster, new InetSocketAddress("localhost", 10000));

        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = httpLoadBalancer.start();
        l4FrontListenerStartupEvent.future().join();
        assertTrue(l4FrontListenerStartupEvent.isSuccessful());

        // Send using HTTP/1.1
        HttpRequest httpRequest = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("https://localhost:20000"))
                .version(HttpClient.Version.HTTP_1_1)
                .timeout(Duration.ofSeconds(5))
                .build();

        HttpResponse<String> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(200, httpResponse.statusCode());
        assertEquals("Meow", httpResponse.body());

        // Send using HTTP/2
        httpRequest = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("https://localhost:20000"))
                .version(HttpClient.Version.HTTP_2)
                .timeout(Duration.ofSeconds(5))
                .build();

        httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(200, httpResponse.statusCode());
        assertEquals("Meow", httpResponse.body());

        // Shutdown HTTP Server and Load Balancer
        httpServer.shutdown();
        L4FrontListenerStopEvent l4FrontListenerStopEvent = httpLoadBalancer.stop();
        l4FrontListenerStopEvent.future().join();
        assertTrue(l4FrontListenerStopEvent.isSuccessful());
    }

    @Test
    void http1BackendWithTLSClient() throws Exception {
        HTTPServer httpServer = new HTTPServer(10001, false);
        httpServer.start();
        Thread.sleep(500L);

        Cluster cluster = new ClusterPool(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE));

        HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withCoreConfiguration(CoreConfiguration.DEFAULT)
                .withHTTPConfiguration(HTTPConfiguration.DEFAULT)
                .withTLSForServer(forServer)
                .withBindAddress(new InetSocketAddress("localhost", 20001))
                .withHTTPInitializer(new DefaultHTTPServerInitializer())
                .withL4FrontListener(new TCPListener())
                .build();

        httpLoadBalancer.mapCluster("localhost:20001", cluster);
        new Node(cluster, new InetSocketAddress("localhost", 10001));

        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = httpLoadBalancer.start();
        l4FrontListenerStartupEvent.future().join();
        assertTrue(l4FrontListenerStartupEvent.isSuccessful());

        // Send using HTTP/1.1
        HttpRequest httpRequest = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("https://localhost:20001"))
                .version(HttpClient.Version.HTTP_2)
                .timeout(Duration.ofSeconds(5))
                .build();

        HttpResponse<String> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(200, httpResponse.statusCode());
        assertEquals("Meow", httpResponse.body());

        // Send using HTTP/2
        httpRequest = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("https://localhost:20001"))
                .version(HttpClient.Version.HTTP_1_1)
                .timeout(Duration.ofSeconds(5))
                .build();

        httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(200, httpResponse.statusCode());
        assertEquals("Meow", httpResponse.body());

        // Shutdown HTTP Server and Load Balancer
        httpServer.shutdown();
        L4FrontListenerStopEvent l4FrontListenerStopEvent = httpLoadBalancer.stop();
        l4FrontListenerStopEvent.future().join();
        assertTrue(l4FrontListenerStopEvent.isSuccessful());
    }

    @Test
    void http2BackendWithoutTLSClient() throws Exception {
        HTTPServer httpServer = new HTTPServer(10002, true);
        httpServer.start();
        Thread.sleep(500L);

        Cluster cluster = new ClusterPool(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE));

        HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withCoreConfiguration(CoreConfiguration.DEFAULT)
                .withHTTPConfiguration(HTTPConfiguration.DEFAULT)
                .withTLSForClient(forClient)
                .withBindAddress(new InetSocketAddress("localhost", 20002))
                .withHTTPInitializer(new DefaultHTTPServerInitializer())
                .withL4FrontListener(new TCPListener())
                .build();

        httpLoadBalancer.mapCluster("localhost:20002", cluster);
        new Node(cluster, new InetSocketAddress("localhost", 10002));

        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = httpLoadBalancer.start();
        l4FrontListenerStartupEvent.future().join();
        assertTrue(l4FrontListenerStartupEvent.isSuccessful());

        // Send using HTTP/1.1
        HttpRequest httpRequest = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("http://localhost:20002"))
                .version(HttpClient.Version.HTTP_1_1)
                .timeout(Duration.ofSeconds(5))
                .build();

        HttpResponse<String> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(200, httpResponse.statusCode());
        assertEquals("Meow", httpResponse.body());

        // Shutdown HTTP Server and Load Balancer
        httpServer.shutdown();
        L4FrontListenerStopEvent l4FrontListenerStopEvent = httpLoadBalancer.stop();
        l4FrontListenerStopEvent.future().join();
        assertTrue(l4FrontListenerStopEvent.isSuccessful());
    }

    @Test
    void http1BackendWithoutTLSClient() throws Exception {
        HTTPServer httpServer = new HTTPServer(55555, false);
        httpServer.start();
        Thread.sleep(500L);

        Cluster cluster = new ClusterPool(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE));

        HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withCoreConfiguration(CoreConfiguration.DEFAULT)
                .withHTTPConfiguration(HTTPConfiguration.DEFAULT)
                .withBindAddress(new InetSocketAddress("localhost", 20003))
                .withHTTPInitializer(new DefaultHTTPServerInitializer())
                .withL4FrontListener(new TCPListener())
                .build();

        httpLoadBalancer.mapCluster("localhost:20003", cluster);
        new Node(cluster, new InetSocketAddress("localhost", 55555));

        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = httpLoadBalancer.start();
        l4FrontListenerStartupEvent.future().join();
        assertTrue(l4FrontListenerStartupEvent.isSuccessful());

        // Send using HTTP/1.1
        HttpRequest httpRequest = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("http://localhost:20003"))
                .version(HttpClient.Version.HTTP_1_1)
                .timeout(Duration.ofSeconds(5))
                .build();

        HttpResponse<String> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(200, httpResponse.statusCode());
        assertEquals("Meow", httpResponse.body());

        // Shutdown HTTP Server and Load Balancer
        httpServer.shutdown();
        L4FrontListenerStopEvent l4FrontListenerStopEvent = httpLoadBalancer.stop();
        l4FrontListenerStopEvent.future().join();
        assertTrue(l4FrontListenerStopEvent.isSuccessful());
    }
}
