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
package com.shieldblaze.expressgateway.integration.aws;

import com.shieldblaze.expressgateway.integration.Fleet;
import com.shieldblaze.expressgateway.integration.Server;
import com.shieldblaze.expressgateway.integration.event.FleetScaleInEvent;
import com.shieldblaze.expressgateway.integration.event.FleetScaleOutEvent;
import software.amazon.awssdk.auth.credentials.AwsCredentialsProvider;
import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.lightsail.LightsailClient;
import software.amazon.awssdk.services.lightsail.model.GetInstancesRequest;
import software.amazon.awssdk.services.lightsail.model.GetInstancesResponse;
import software.amazon.awssdk.services.lightsail.model.Instance;

import java.net.UnknownHostException;
import java.util.ArrayList;
import java.util.List;
import java.util.stream.Collectors;

public class LightsailFleet extends AWS implements Fleet, Runnable {

    private final List<Server> servers = new ArrayList<>();
    private final LightsailClient lightsailClient;

    protected LightsailFleet(AwsCredentialsProvider awsCredentialsProvider, Region region) {
        super(awsCredentialsProvider, region);

        lightsailClient = LightsailClient.builder()
                .credentialsProvider(awsCredentialsProvider)
                .region(region)
                .build();
    }

    @Override
    public List<Server> servers() {
        return servers(null);
    }

    private List<Server> servers(String pageToken) {
        GetInstancesResponse getInstancesResponse;
        if (pageToken == null) {
            getInstancesResponse = lightsailClient.getInstances();
        } else {
            getInstancesResponse = lightsailClient.getInstances(GetInstancesRequest.builder().pageToken(pageToken).build());
        }

        for (Instance instance : getInstancesResponse.instances()) {
            try {
                servers.add(LightsailServer.buildFrom(instance));
            } catch (UnknownHostException ex) {
                // Ignore as this is never gonna happen because AWS can't have wrong IP address.
            }
        }

        if (getInstancesResponse.nextPageToken() != null) {
            servers(getInstancesResponse.nextPageToken());
        }

        return servers;
    }

    @Override
    public FleetScaleInEvent<?> scaleIn(int count) {
        return null;
    }

    @Override
    public FleetScaleOutEvent<?> scaleOut(int count) {
        return null;
    }

    @Override
    public void run() {

    }
}
