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

import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.common.utils.SelfSignedCertificate;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.configuration.tls.CertificateKeyPair;
import com.shieldblaze.expressgateway.configuration.tls.TlsClientConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TlsServerConfiguration;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import io.netty.handler.codec.http.HttpHeaderNames;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.util.List;

import static org.assertj.core.api.Assertions.assertThat;

public class WebsiteProxyTest {

    private static final List<String> WEBSITES = List.of(
            "www.facebook.com",
            "www.amazon.com",
            "www.google.com",
            "www.microsoft.com",
            "www.youtube.com",
            "www.instagram.com",
            "www.cloudflare.com",
            "www.netflix.com",
            "www.twitter.com",
            "www.shieldblaze.com"
    );

    private static HTTPLoadBalancer httpLoadBalancer;

    @BeforeAll
    static void setup() throws Exception {
        String Hostname = "www.shieldblaze.com";

        SelfSignedCertificate ssc = SelfSignedCertificate.generateNew(List.of("127.0.0.1"), List.of("localhost"));
        CertificateKeyPair certificateKeyPair = CertificateKeyPair.forClient(List.of(ssc.x509Certificate()), ssc.keyPair().getPrivate());

        TlsClientConfiguration tlsClientConfiguration = TlsClientConfiguration.DEFAULT;
        TlsServerConfiguration tlsServerConfiguration = TlsServerConfiguration.DEFAULT;

        tlsServerConfiguration.enable();
        tlsServerConfiguration.addMapping("localhost", certificateKeyPair);
        tlsServerConfiguration.defaultMapping(certificateKeyPair);

        tlsClientConfiguration.enable();
        tlsClientConfiguration.setAcceptAllCerts(true);
        tlsClientConfiguration.defaultMapping(CertificateKeyPair.newDefaultClientInstance());

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withConfigurationContext(ConfigurationContext.create(tlsClientConfiguration, tlsServerConfiguration))
                .withBindAddress(new InetSocketAddress("0.0.0.0", 9110))
                .withHTTPInitializer(new DefaultHTTPServerInitializer())
                .withL4FrontListener(new TCPListener())
                .build();

        httpLoadBalancer.defaultCluster(cluster);

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress(Hostname, 443))
                .build();

        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = httpLoadBalancer.start();
        l4FrontListenerStartupEvent.future().get();
    }

    @AfterAll
    static void shutdown() throws Exception {
        httpLoadBalancer.shutdown().future().get();
    }

    @Test
    void loadWebsitesAndExpect200Test() throws Exception {
        HttpClient httpClient = TestableHttpLoadBalancer.httpClient();

        for (String domain : WEBSITES) {
            HttpRequest httpRequest = HttpRequest.newBuilder()
                    .GET()
                    .setHeader(HttpHeaderNames.USER_AGENT.toString(),
                            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/106.0.0.0 Safari/537.36")
                    .uri(URI.create("https://" + domain + '/'))
                    .build();

            HttpResponse<Void> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.discarding());
            assertThat(httpResponse.statusCode()).isEqualTo(200);
        }
    }
}
