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
package com.shieldblaze.expressgateway.configuration.http;

import com.fasterxml.jackson.annotation.JsonIgnore;
import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.common.utils.NumberUtil;
import com.shieldblaze.expressgateway.configuration.Configuration;
import dev.morphia.annotations.Entity;
import dev.morphia.annotations.Id;
import dev.morphia.annotations.Property;
import dev.morphia.annotations.Transient;

import java.util.Objects;
import java.util.UUID;

/**
 * Configuration for HTTP
 */
@Entity(value = "Http", useDiscriminator = false)
public final class HttpConfiguration implements Configuration<HttpConfiguration> {

    @Id
    @JsonProperty
    private String id;

    @Property
    @JsonProperty
    private String profileName;

    @Property
    @JsonProperty
    private long maxContentLength;

    @Property
    @JsonProperty
    private int maxInitialLineLength;

    @Property
    @JsonProperty
    private int maxHeaderSize;

    @Property
    @JsonProperty
    private int maxChunkSize;

    @Property
    @JsonProperty
    private int h2InitialWindowSize;

    @Property
    @JsonProperty
    private long h2MaxConcurrentStreams;

    @Property
    @JsonProperty
    private long h2MaxHeaderListSize;

    @Property
    @JsonProperty
    private long h2MaxHeaderTableSize;

    @Property
    @JsonProperty
    private int h2MaxFrameSize;

    @Property
    @JsonProperty
    private int compressionThreshold;

    @Property
    @JsonProperty
    private int deflateCompressionLevel;

    @Property
    @JsonProperty
    private int brotliCompressionLevel;

    @Transient
    @JsonIgnore
    private boolean validated;

    public static final HttpConfiguration DEFAULT = new HttpConfiguration();

    static {
        DEFAULT.id = "default";
        DEFAULT.profileName = "default";
        DEFAULT.maxContentLength = 500000000;
        DEFAULT.h2InitialWindowSize = 65535;
        DEFAULT.h2MaxConcurrentStreams = 1000;
        DEFAULT.h2MaxHeaderListSize = 262144;
        DEFAULT.h2MaxHeaderTableSize = 65536;
        DEFAULT.h2MaxFrameSize = 16777215;
        DEFAULT.maxInitialLineLength = 1024 * 8;
        DEFAULT.maxHeaderSize = 1024 * 8;
        DEFAULT.maxChunkSize = 1024 * 8;
        DEFAULT.compressionThreshold = 1024;
        DEFAULT.deflateCompressionLevel = 6;
        DEFAULT.brotliCompressionLevel = 4;
        DEFAULT.validated = true;
    }

    HttpConfiguration() {
        // Prevent outside initialization
    }

    /**
     * Profile name
     */
    public HttpConfiguration setProfileName(String profileName) {
        this.profileName = profileName;
        return this;
    }

    /**
     * Profile name
     */
    @Override
    public String profileName() {
        assertValidated();
        return profileName;
    }

    /**
     * Max Content Length
     */
    public HttpConfiguration setMaxContentLength(long maxContentLength) {
        this.maxContentLength = maxContentLength;
        return this;
    }

    /**
     * Max Content Length
     */
    public long maxContentLength() {
        assertValidated();
        return maxContentLength;
    }

    /**
     * Max Initial Line Length
     */
    public HttpConfiguration setMaxInitialLineLength(int maxInitialLineLength) {
        this.maxInitialLineLength = maxInitialLineLength;
        return this;
    }

    /**
     * Max Initial Line Length
     */
    public int maxInitialLineLength() {
        assertValidated();
        return maxInitialLineLength;
    }

    /**
     * Max Header Size
     */
    public HttpConfiguration setMaxHeaderSize(int maxHeaderSize) {
        this.maxHeaderSize = maxHeaderSize;
        return this;
    }

    /**
     * Max Header Size
     */
    public int maxHeaderSize() {
        assertValidated();
        return maxHeaderSize;
    }

    /**
     * Max Chunk Size
     */
    public HttpConfiguration setMaxChunkSize(int maxChunkSize) {
        this.maxChunkSize = maxChunkSize;
        return this;
    }

    /**
     * Max Chunk Size
     */
    public int maxChunkSize() {
        assertValidated();
        return maxChunkSize;
    }

    /**
     * HTTP/2 Initial Window Size
     */
    public HttpConfiguration setH2InitialWindowSize(int h2InitialWindowSize) {
        this.h2InitialWindowSize = h2InitialWindowSize;
        return this;
    }

    /**
     * HTTP/2 Initial Window Size
     */
    public int h2InitialWindowSize() {
        assertValidated();
        return h2InitialWindowSize;
    }

    /**
     * HTTP/2 Max Concurrent Streams
     */
    public HttpConfiguration setH2MaxConcurrentStreams(long h2MaxConcurrentStreams) {
        this.h2MaxConcurrentStreams = h2MaxConcurrentStreams;
        return this;
    }

    /**
     * HTTP/2 Max Concurrent Streams
     */
    public long h2MaxConcurrentStreams() {
        assertValidated();
        return h2MaxConcurrentStreams;
    }

    /**
     * HTTP/2 Max Header List Size
     */
    public HttpConfiguration setH2MaxHeaderListSize(long h2MaxHeaderSizeList) {
        this.h2MaxHeaderListSize = h2MaxHeaderSizeList;
        return this;
    }

    /**
     * HTTP/2 Max Header List Size
     */
    public long h2MaxHeaderListSize() {
        assertValidated();
        return h2MaxHeaderListSize;
    }

    /**
     * HTTP/2 Max Header Table Size
     */
    public HttpConfiguration setH2MaxHeaderTableSize(long h2MaxHeaderTableSize) {
        this.h2MaxHeaderTableSize = h2MaxHeaderTableSize;
        return this;
    }

    /**
     * HTTP/2 Max Header Table Size
     */
    public long h2MaxHeaderTableSize() {
        assertValidated();
        return h2MaxHeaderTableSize;
    }

    /**
     * HTTP/2 Max Frame Size
     */
    public HttpConfiguration setH2MaxFrameSize(int h2MaxFrameSize) {
        this.h2MaxFrameSize = h2MaxFrameSize;
        return this;
    }

    /**
     * HTTP/2 Max Frame Size
     */
    public int h2MaxFrameSize() {
        assertValidated();
        return h2MaxFrameSize;
    }

    /**
     * Compression Threshold
     */
    public HttpConfiguration setCompressionThreshold(int compressionThreshold) {
        this.compressionThreshold = compressionThreshold;
        return this;
    }

    /**
     * Compression Threshold
     */
    public int compressionThreshold() {
        assertValidated();
        return compressionThreshold;
    }

    /**
     * Deflate Compression Level
     */
    public HttpConfiguration setDeflateCompressionLevel(int deflateCompressionLevel) {
        this.deflateCompressionLevel = deflateCompressionLevel;
        return this;
    }

    public int deflateCompressionLevel() {
        assertValidated();
        return deflateCompressionLevel;
    }

    /**
     * Brotli Compression Level
     */
    public HttpConfiguration setBrotliCompressionLevel(int brotliCompressionLevel) {
        this.brotliCompressionLevel = brotliCompressionLevel;
        return this;
    }

    /**
     * Brotli Compression Level
     */
    public int brotliCompressionLevel() {
        assertValidated();
        return brotliCompressionLevel;
    }

    /**
     * Validate all parameters of this configuration
     *
     * @return this class instance
     * @throws IllegalArgumentException If any value is invalid
     */
    public HttpConfiguration validate() throws IllegalArgumentException {
        if (id == null) {
            id = UUID.randomUUID().toString();
        }
        Objects.requireNonNull(profileName, "Profile Name");
        NumberUtil.checkPositive(maxContentLength, "maxContentLength");
        NumberUtil.checkPositive(h2InitialWindowSize, "h2InitialWindowSize");
        NumberUtil.checkPositive(h2MaxConcurrentStreams, "h2MaxConcurrentStreams");
        NumberUtil.checkPositive(h2MaxHeaderListSize, "h2MaxHeaderListSize");
        NumberUtil.checkPositive(h2MaxHeaderTableSize, "h2MaxHeaderTableSize");
        NumberUtil.checkPositive(h2MaxFrameSize, "h2MaxFrameSize");
        NumberUtil.checkPositive(maxInitialLineLength, "maxInitialLineLength");
        NumberUtil.checkPositive(maxHeaderSize, "maxHeaderSize");
        NumberUtil.checkPositive(maxChunkSize, "maxChunkSize");
        NumberUtil.checkZeroOrPositive(compressionThreshold, "compressionThreshold");
        NumberUtil.checkInRange(deflateCompressionLevel, 0, 9, "deflateCompressionLevel");
        NumberUtil.checkInRange(brotliCompressionLevel, 1, 11, "brotliCompressionLevel");
        validated = true;
        return this;
    }

    @Override
    public String id() {
        return id;
    }

    @Override
    public boolean validated() {
        return validated;
    }
}
