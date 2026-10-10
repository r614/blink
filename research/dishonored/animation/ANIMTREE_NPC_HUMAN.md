# `NPC_Human_at`: the cooked AnimTree, decoded

Output of `npc_anim_dump tree Startup Npc_Common_at.NPC_Human_at` (`dishonored_assets::anim_tree::load_anim_tree`): the structure research/dishonored/ANIMATION.md section 2 describes. A node reached from several parents is printed once and referenced afterwards.

```
AnimTree: Npc_Common_at.NPC_Human_at
AnimNodeBlendByPhysics "NPCPhysicsBlend"
  PHYS_None
  DisAnimNodeSlot "AdditiveReactSlot"
    Source
    AnimNodeAdditiveBlending "AnimNodeAdditiveBlending_31"
      Base Anim Input
      ArkAnimNodeBlendPose "PoseBlender"
        Child
        ArkAnimNodeStack "MatineeStackNode"
          Source
          AnimNodeBlendPerBone "FaceFilter" branches=["head_jnt"] (exclude)
            Source
            ArkAnimNodeStack "BlinkBlender"
              Source
              AnimNodeBlendPerBone "UpperBodyFilter" branches=["spine_0_jnt"]
                Source
                DisAnimNodeSlot "FullBodySlot"
                  Source
                  ArkAnimNodeLocomotion "LocoBlender" rate=2000 range=100 min_ratio=0.1
                    SpeedModeSlot0
                    DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_12" seq="Empty_Idle" rate=1 (loop)
                    SpeedModeSlot1
                    DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_37"
                    SpeedModeSlot2
                    DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_38"
                    SpeedModeSlot3
                    DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_39"
                    SpeedModeSlot4
                    DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_40"
                    SpeedModeSlot5
                    DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_41"
                    SpeedModeSlot6
                    DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_42"
                    SpeedModeSlot7
                    DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_43"
                    Specific anim
                    DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_2"
                  Channel  0
                  DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_17" seq="Sword_DodgeLeftIn" rate=1
                  Channel  1
                  DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_18" seq="Sword_DodgeLeftOut" rate=1
                  Channel  2
                  DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_15" seq="Beg_Kneeling_Out" rate=1
                Target
                DisAnimNodeSlot "UpperBodySlot"
                  Source
                  (shared #13 "FullBodySlot")
                  Channel  0
                  DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_48" seq="Sword_Equip" rate=1
                  Channel  1
                  DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_46" seq="Beg_Kneeling_Give_In" rate=1
                  Channel  2
                  DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_203"
              Channel 01
              AnimNodeSequence "Blink" seq="Blink" rate=1
            Target
            AnimNodeSlot "FaceSlot"
              Source
              (shared #20 "BlinkBlender")
              Channel  0
              DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_13"
              Channel  1
              DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_1"
          Channel  0
          DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_204"
          Channel  1
          DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_205"
          Channel  2
          DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_206"
          Channel  3
          DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_207"
          Channel  4
          DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_208"
          Channel  5
          DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_209"
      Additive Anim Input [additive]
      ArkAnimNodeLookAt "LookatBlender" grids=["ADD_Empty_AimHeadIdle_UpLeft", "ADD_Empty_AimHeadIdle_CenterLeft", "ADD_Empty_AimHeadIdle_DownLeft", "ADD_Empty_AimHeadIdle_Up", "ADD_Empty_AimHeadIdle_Center", "ADD_Empty_AimHeadIdle_Down", "ADD_Empty_AimHeadIdle_UpRight", "ADD_Empty_AimHeadIdle_CenterRight", "ADD_Empty_AimHeadIdle_DownRight", "ADD_Empty_AimTorsoIdle_UpLeft", "ADD_Empty_AimTorsoIdle_CenterLeft", "ADD_Empty_AimTorsoIdle_DownLeft", "ADD_Empty_AimTorsoIdle_Up", "ADD_Empty_AimTorsoIdle_Center", "ADD_Empty_AimTorsoIdle_Down", "ADD_Empty_AimTorsoIdle_UpRight", "ADD_Empty_AimTorsoIdle_CenterRight", "ADD_Empty_AimTorsoIdle_DownRight"]
    Channel  0
    DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_47" seq="Sword_Surprised_Front" rate=1
  PHYS_Walking
  (shared #36 "AdditiveReactSlot")
  PHYS_Falling
  (shared #36 "AdditiveReactSlot")
  PHYS_Swimming
  (shared #36 "AdditiveReactSlot")
  PHYS_Flying
  (shared #36 "AdditiveReactSlot")
  PHYS_Rotating
  (shared #36 "AdditiveReactSlot")
  PHYS_Projectile
  (shared #36 "AdditiveReactSlot")
  PHYS_Interpolating
  (shared #36 "AdditiveReactSlot")
  PHYS_Spider
  (shared #36 "AdditiveReactSlot")
  PHYS_Ladder
  (shared #36 "AdditiveReactSlot")
  PHYS_RigidBody
  DishonoredAnimNodeSeq "DishonoredAnimNodeSeq_210"
  PHYS_SoftBody
  (shared #36 "AdditiveReactSlot")
  PHYS_NavMeshWalking
  (shared #36 "AdditiveReactSlot")
  PHYS_Unused
  (shared #36 "AdditiveReactSlot")
  PHYS_Custom
  (shared #36 "AdditiveReactSlot")

Skeletal controls:
  spine_0_jnt: DisSkelControl_SpineBender "SpineBender"
  eye_L_jnt: SkelControlSingleBone "EyeControl_Left"
  eye_R_jnt: SkelControlSingleBone "EyeControl_Right"
  foot_R_jnt: DisSkelControl_FootPlacement "FootPlacement_Right"
  foot_L_jnt: DisSkelControl_FootPlacement "FootPlacement_Left"
```
