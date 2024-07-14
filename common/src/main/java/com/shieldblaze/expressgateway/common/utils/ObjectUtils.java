package com.shieldblaze.expressgateway.common.utils;

public final class ObjectUtils {

    private ObjectUtils() {
        // Prevent outside initialization
    }

    public static <T> T nonNull(T obj, Class<?> clazz) {
        return nonNull(obj, clazz.getSimpleName());
    }

    public static <T> T nonNull(T obj, String message) {
        if (obj == null) {
            throw new NullPointerException(message);
        }
        return obj;
    }
}
