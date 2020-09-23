module Style.Autoprefixer exposing (..)

import Css exposing (Style, property)


legacyDisplayGrid : Style
legacyDisplayGrid =
    Css.batch
        [ property "display" "-ms-grid"
        , property "display" "grid"
        ]


legacyGridTemplateColumns : String -> Style
legacyGridTemplateColumns value =
    Css.batch
        [ property "-ms-grid-columns" value
        , property "grid-template-columns" value
        ]


legacyGridTemplateRows : String -> Style
legacyGridTemplateRows value =
    Css.batch
        [ property "-ms-grid-rows" value
        , property "grid-template-rows" value
        ]


legacyGridColumn : String -> Style
legacyGridColumn value =
    Css.batch
        [ property "-ms-grid-column" value
        , property "grid-column" value
        ]


legacyGridRow : String -> Style
legacyGridRow value =
    Css.batch
        [ property "-ms-grid-row" value
        , property "grid-row" value
        ]


legacyTransform : String -> Style
legacyTransform translate =
    Css.batch
        [ property "-webkit-transform" translate
        , property "transform" translate
        ]


legacyTransition : String -> Style
legacyTransition transition =
    Css.batch
        [ property "-webkit-transition" transition
        , property "transition" transition
        ]


legacyUserSelect : String -> Style
legacyUserSelect value =
    Css.batch
        [ property "-webkit-user-select" value
        , property "-moz-user-select" value
        , property "-ms-user-select" value
        , property "user-select" value
        ]


legacyBoxShadow : String -> Style
legacyBoxShadow value =
    Css.batch
        [ property "-webkit-box-shadow" value
        , property "box-shadow" value
        ]


legacyBoxSizing : String -> Style
legacyBoxSizing value =
    Css.batch
        [ property "-webkit-box-sizing" value
        , property "box-sizing" value
        ]


legacyAlignItems : String -> Style
legacyAlignItems value =
    Css.batch
        [ property "-webkit-box-align" value
        , property "-ms-flex-align" value
        , property "align-items" value
        ]


legacyTransitionDelay : String -> Style
legacyTransitionDelay value =
    Css.batch
        [ property "-webkit-transition-delay" value
        , property "transition-delay" value
        ]


legacyDisplayFlex : Style
legacyDisplayFlex =
    Css.batch
        [ property "display" "-webkit-box"
        , property "display" "-ms-flexbox"
        , property "display" "flex"
        ]


legacyJustifyContentSpaceBetween : Style
legacyJustifyContentSpaceBetween =
    Css.batch
        [ property "-webkit-box-pack" "justify"
        , property "-ms-flex-pack" "justify"
        , property "justify-content" "space-between"
        ]


legacyJustifyContentSpaceAround : Style
legacyJustifyContentSpaceAround =
    Css.batch
        [ property "-ms-flex-pack" "distribute"
        , property "justify-content" "space-around"
        ]


appearance : String -> Style
appearance value =
    Css.batch
        [ property "-webkit-appearance" value
        , property "-moz-appearance" value
        ]
