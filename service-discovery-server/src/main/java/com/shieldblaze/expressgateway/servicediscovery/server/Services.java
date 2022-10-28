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
package com.shieldblaze.expressgateway.servicediscovery.server;

import com.shieldblaze.expressgateway.common.utils.SystemPropertyUtil;
import com.shieldblaze.expressgateway.common.zookeeper.Environment;
import com.shieldblaze.expressgateway.common.zookeeper.ZNodePath;
import org.apache.curator.framework.CuratorFramework;
import org.apache.curator.framework.CuratorFrameworkFactory;
import org.apache.curator.retry.RetryNTimes;
import org.apache.curator.x.discovery.ServiceDiscovery;
import org.apache.curator.x.discovery.ServiceDiscoveryBuilder;
import org.apache.curator.x.discovery.details.JsonInstanceSerializer;
import org.springframework.boot.ApplicationArguments;
import org.springframework.context.annotation.Bean;
import org.springframework.stereotype.Component;

import static com.shieldblaze.expressgateway.common.zookeeper.Environment.DEVELOPMENT;
import static com.shieldblaze.expressgateway.common.zookeeper.Environment.PRODUCTION;
import static com.shieldblaze.expressgateway.common.zookeeper.Environment.TESTING;
import static com.shieldblaze.expressgateway.servicediscovery.server.ServiceDiscoveryServer.SERVICE_NAME;

@Component
public class Services {

    private final ApplicationArguments applicationArguments;

    public Services(ApplicationArguments applicationArguments) {
        this.applicationArguments = applicationArguments;
    }

    @Bean(destroyMethod = "close")
    public ServiceDiscovery<Node> serviceDiscovery() throws Exception {
        CuratorFramework curatorFramework = CuratorFrameworkFactory.newClient(applicationArguments.getSourceArgs()[0], new RetryNTimes(3, 100));
        curatorFramework.start();

        String path = ZNodePath.create(SERVICE_NAME,
                detectEnv(),
                SystemPropertyUtil.getPropertyOrEnv("ClusterID"),
                "ServiceDiscovery",
                "Members").path();

        ServiceDiscovery<Node> serviceDiscovery = ServiceDiscoveryBuilder.builder(Node.class)
                .client(curatorFramework)
                .basePath(path)
                .serializer(new JsonInstanceSerializer<>(Node.class))
                .build();

        serviceDiscovery.start();
        return serviceDiscovery;
    }

    /**
     * Automatically detect environment.
     * Default is: {@link Environment#TESTING}
     *
     * @return {@link Environment} type
     */
    public static Environment detectEnv() {
        String env = SystemPropertyUtil.getPropertyOrEnv("runtime.env", "test").toLowerCase();
        return switch (env) {
            case "dev", "development" -> DEVELOPMENT;
            case "test", "quality-assurance" -> TESTING;
            case "prod", "production" -> PRODUCTION;
            default -> null;
        };
    }
}
