# ROS 2 wire compatibility

## Goal

Qualify bounded ROS 2 wire interoperability without implying support beyond the
admitted profile. Work-item state lives in `.tasks/items/`.

## Release boundary

R0 defines the minimum topic path; R1/R2 broaden it to external `rmw_zenoh`
peers, services, and actions. Transport-level security is not claimed by
R0/R1/R2 unless separately admitted and verified.

## Owning references

- [`../plans/rpi5-ros2-demo.md`](../plans/rpi5-ros2-demo.md)
- [`../../roadmap/03-ros2-compatibility.md`](../../roadmap/03-ros2-compatibility.md)
