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
package com.shieldblaze.expressgateway.integration.aws.lightsail.instance;

import com.shieldblaze.expressgateway.integration.aws.AWS;
import com.shieldblaze.expressgateway.integration.event.FleetScaleInEvent;
import com.shieldblaze.expressgateway.integration.event.FleetScaleOutEvent;
import com.shieldblaze.expressgateway.integration.server.FleetManager;
import com.shieldblaze.expressgateway.integration.server.Server;
import software.amazon.awssdk.auth.credentials.AwsCredentialsProvider;
import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.lightsail.LightsailClient;
import software.amazon.awssdk.services.lightsail.model.CreateInstancesResponse;
import software.amazon.awssdk.services.lightsail.model.DeleteInstanceResponse;
import software.amazon.awssdk.services.lightsail.model.GetInstancesRequest;
import software.amazon.awssdk.services.lightsail.model.GetInstancesResponse;
import software.amazon.awssdk.services.lightsail.model.Instance;

import java.io.Closeable;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

/**
 * Lightsail Fleet Manager
 */
public final class LightsailFleetManager extends AWS implements FleetManager<Server, ScaleOutRequest>, Runnable, Closeable {

    private final ScheduledExecutorService executorService = Executors.newSingleThreadScheduledExecutor();
    private final ScheduledFuture<?> scheduledFuture;

    private final List<Server> servers = new CopyOnWriteArrayList<>();
    private final LightsailClient lightsailClient;
    private final LightsailScale lightsailScale;

    public LightsailFleetManager(AwsCredentialsProvider awsCredentialsProvider, Region region) {
        super(awsCredentialsProvider, region);

        lightsailClient = LightsailClient.builder()
                .credentialsProvider(awsCredentialsProvider)
                .region(region)
                .build();

        lightsailScale = new LightsailScale(lightsailClient);
        scheduledFuture = executorService.scheduleAtFixedRate(this, 0, 10, TimeUnit.SECONDS);
    }

    @Override
    public void run() {
        List<Server> serverList = new ArrayList<>();
        servers(serverList, null);

        servers.clear();
        servers.addAll(serverList);
    }

    @Override
    public List<Server> servers() {
        return servers;
    }

    private void servers(List<Server> serverList, String pageToken) {
        GetInstancesResponse getInstancesResponse;
        if (pageToken == null) {
            getInstancesResponse = lightsailClient.getInstances();
        } else {
            getInstancesResponse = lightsailClient.getInstances(GetInstancesRequest.builder().pageToken(pageToken).build());
        }

        for (Instance instance : getInstancesResponse.instances()) {
                serverList.add(LightsailInstance.buildFrom(lightsailClient, instance));
        }

        if (getInstancesResponse.nextPageToken() != null) {
            servers(serverList, getInstancesResponse.nextPageToken());
        }
    }

    @Override
    public FleetScaleInEvent<DeleteInstanceResponse> scaleIn(Server server) {
        return lightsailScale.scaleIn(server.id());
    }

    @Override
    public FleetScaleOutEvent<CreateInstancesResponse> scaleOut(ScaleOutRequest scaleOutRequest) {
        return lightsailScale.scaleOut(scaleOutRequest);
    }

    @Override
    public void close() {
        scheduledFuture.cancel(true);
        executorService.shutdown();
        lightsailClient.close();
    }
}
