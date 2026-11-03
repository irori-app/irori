/**
 * The numbered points on the home page's library screenshot, in percent of its
 * width and height. Their titles and texts are in the dictionaries (`hotspots`),
 * in the same order. To use another screenshot, change `screen` and move the points.
 */
import type { ScreenId } from './site'

export const TOUR: { screen: ScreenId; points: { x: number; y: number }[] } = {
  screen: 'library',
  points: [
    { x: 5.6, y: 14.2 },
    { x: 46.5, y: 1.9 },
    { x: 49, y: 17.4 },
    { x: 24, y: 37 },
    { x: 58, y: 70 },
    { x: 96.3, y: 17.4 },
    { x: 13.5, y: 96.4 },
  ],
}
