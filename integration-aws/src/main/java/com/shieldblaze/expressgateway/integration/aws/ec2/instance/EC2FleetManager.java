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
package com.shieldblaze.expressgateway.integration.aws.ec2.instance;

import com.shieldblaze.expressgateway.integration.server.FleetManager;
import com.shieldblaze.expressgateway.integration.server.Server;
import com.shieldblaze.expressgateway.integration.aws.AWS;
import com.shieldblaze.expressgateway.integration.event.FleetScaleInEvent;
import com.shieldblaze.expressgateway.integration.event.FleetScaleOutEvent;
import software.amazon.awssdk.auth.credentials.AwsCredentialsProvider;
import software.amazon.awssdk.regions.Region;
import software.amazon.awssdk.services.ec2.Ec2Client;
import software.amazon.awssdk.services.ec2.model.DescribeInstancesRequest;
import software.amazon.awssdk.services.ec2.model.DescribeInstancesResponse;
import software.amazon.awssdk.services.ec2.model.Instance;
import software.amazon.awssdk.services.ec2.model.Reservation;
import software.amazon.awssdk.services.ec2.model.TerminateInstancesResponse;

import java.io.Closeable;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

/**
 * EC2 Fleet Manager
 */
public final class EC2FleetManager extends AWS implements FleetManager<Server, ScaleOutRequest>, Runnable, Closeable {

    private final ScheduledExecutorService executorService = Executors.newSingleThreadScheduledExecutor();
    private final ScheduledFuture<?> scheduledFuture;

    private final List<Server> servers = new CopyOnWriteArrayList<>();
    private final Ec2Client ec2Client;
    private final EC2Scale ec2Scale;

    protected EC2FleetManager(AwsCredentialsProvider awsCredentialsProvider, Region region) {
        super(awsCredentialsProvider, region);

        ec2Client = Ec2Client.builder()
                .credentialsProvider(awsCredentialsProvider)
                .region(region)
                .build();

        ec2Scale = new EC2Scale(ec2Client);
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
        DescribeInstancesResponse describeInstancesResponse;
        if (pageToken == null) {
            describeInstancesResponse = ec2Client.describeInstances();
        } else {
            describeInstancesResponse = ec2Client.describeInstances(DescribeInstancesRequest.builder().nextToken(pageToken).build());
        }

        for (Reservation reservation : describeInstancesResponse.reservations()) {
            for (Instance instance : reservation.instances()) {
                serverList.add(EC2Instance.buildFrom(ec2Client, instance));
            }
        }

        if (describeInstancesResponse.nextToken() != null) {
            servers(serverList, describeInstancesResponse.nextToken());
        }
    }

    @Override
    public FleetScaleInEvent<TerminateInstancesResponse> scaleIn(Server server) {
        return ec2Scale.scaleIn((EC2Instance) server);
    }

    @Override
    public FleetScaleOutEvent<Instance> scaleOut(ScaleOutRequest scaleOutRequest) {
        return ec2Scale.scaleOut(scaleOutRequest);
    }

    @Override
    public void close() {
        scheduledFuture.cancel(true);
        executorService.shutdown();
        ec2Client.close();
    }
}
